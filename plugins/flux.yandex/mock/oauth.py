"""Yandex ID's OAuth for the fake Yandex (mock_yandex.py): the code with PKCE, who signed in, and
Yandex 360's list of the user's organizations.

- GET /authorize — answers at once as if the user allowed it: 302 to `redirect_uri` with `code` and
  `state` (`POST /_mock/oauth {"deny": true}` — with `error=access_denied` instead). Without
  `redirect_uri` — an app "for API access or debugging": the verification code page, the code in
  `<code id="code">…</code>` (and `GET /_mock/oauth/code` — the last one).
- POST /token — `grant_type=authorization_code` with `code`, `client_id`, `code_verifier`: the
  verifier's S256 must be the code's challenge, the `redirect_uri` the code's; a token that lives a
  year (`{"expires_in": seconds}` to the control — another lifetime). `grant_type=refresh_token`
  with a refresh token it gave and the same `client_id`, no secret — a new token
  (`{"refresh": false}` — refused, as Yandex may refuse without a secret).
- GET /info — who the token is.
- GET /directory/v1/org — Yandex 360's organizations of the token's user:
  `POST /_mock/orgs {"mode": "one" | "two" | "many" | "empty" | "forbidden"}` (one by default:
  123456, the organization Tracker's part accepts).

Tokens it gave are in `state.data["oauth"]["tokens"]`; Tracker's part accepts them, and pasted
ones that start with `y0_`.
"""
import base64
import hashlib
import secrets
from html import escape
from urllib.parse import urlencode

from mock_yandex import Response

ME = {
    "id": "1120000000016772",
    "login": "egor.ageev",
    "display_name": "Егор Агеев",
    "real_name": "Егор Агеев",
    "default_email": "egor.ageev@example.ru",
    "client_id": "mock",
    "psuid": "1.mock",
}


def challenge(verifier):
    digest = hashlib.sha256(verifier.encode()).digest()
    return base64.urlsafe_b64encode(digest).rstrip(b"=").decode()


ORGS = {
    "one": [{"id": 123456, "name": "Рога и копыта", "email": "admin@roga.example.ru",
             "language": "ru", "subscriptionPlan": "business"}],
    "two": [{"id": 123456, "name": "Рога и копыта"},
            {"id": 654321, "name": "Научно-производственное объединение «Очень длинное название»"}],
    "many": [{"id": 123456, "name": "Рога и копыта"}] + [
        {"id": 700000 + n, "name": f"Организация {n}"} for n in range(1, 6)],
    "empty": [],
}


def register(router, state):
    def reset():
        state.data["oauth"] = {"codes": {}, "tokens": set(), "refresh": {}, "deny": False,
                               "refresh_ok": True, "expires_in": 31536000, "orgs": "one",
                               "last_code": ""}

    reset()
    state.resets.append(reset)
    data = lambda: state.data["oauth"]  # noqa: E731

    def authorize(request, match):
        redirect = request.arg("redirect_uri", "")
        if redirect and not redirect.startswith("http://127.0.0.1:"):
            return Response(400, {"error": "invalid_request",
                                  "error_description": "redirect_uri isn't registered"})
        if request.arg("code_challenge_method") != "S256" or not request.arg("code_challenge"):
            return Response(400, {"error": "invalid_request",
                                  "error_description": "PKCE S256 is expected"})
        if "calendar:all" in request.arg("scope", ""):
            return Response(400, {"error": "invalid_scope",
                                  "error_description": "calendar:all isn't the app's right"})
        state_ = request.arg("state", "")
        if not redirect:
            # A debug app: Yandex shows the code to copy.
            code = str(secrets.randbelow(9000000) + 1000000)
            data()["codes"][code] = {
                "challenge": request.arg("code_challenge"),
                "client_id": request.arg("client_id"),
                "redirect": "",
            }
            data()["last_code"] = code
            page = (f"<!doctype html><html><body><h1>Код подтверждения</h1>"
                    f"<code id=\"code\">{escape(code)}</code></body></html>")
            return Response(200, page, content_type="text/html; charset=utf-8")
        if data()["deny"]:
            query = urlencode({"error": "access_denied",
                               "error_description": "The user denied access", "state": state_})
        else:
            code = secrets.token_hex(8)
            data()["codes"][code] = {
                "challenge": request.arg("code_challenge"),
                "client_id": request.arg("client_id"),
                "redirect": redirect,
            }
            query = urlencode({"code": code, "state": state_})
        return Response(302, b"", headers={"Location": f"{redirect}?{query}"},
                        content_type="text/plain")

    def give(client_id):
        access = "y0_mock_" + secrets.token_hex(6)
        refresh = "1:mock:" + secrets.token_hex(6)
        data()["tokens"].add(access)
        data()["refresh"][refresh] = client_id
        return Response(200, {"access_token": access, "refresh_token": refresh,
                              "expires_in": data()["expires_in"], "token_type": "bearer"})

    def token(request, match):
        form = request.form()
        if form.get("grant_type") == "refresh_token":
            client = data()["refresh"].get(form.get("refresh_token", ""))
            if not data()["refresh_ok"]:
                return Response(400, {"error": "invalid_client",
                                      "error_description": "Client secret is required"})
            if client is None or client != form.get("client_id"):
                return Response(400, {"error": "invalid_grant",
                                      "error_description": "Refresh token is invalid"})
            return give(client)
        if form.get("grant_type") != "authorization_code":
            return Response(400, {"error": "unsupported_grant_type"})
        code = data()["codes"].pop(form.get("code", ""), None)
        if not code:
            return Response(400, {"error": "invalid_grant",
                                  "error_description": "Code has expired"})
        if code["client_id"] != form.get("client_id"):
            return Response(400, {"error": "invalid_client"})
        if code["redirect"] and form.get("redirect_uri") != code["redirect"]:
            return Response(400, {"error": "invalid_grant",
                                  "error_description": "redirect_uri doesn't match"})
        if challenge(form.get("code_verifier", "")) != code["challenge"]:
            return Response(400, {"error": "invalid_grant",
                                  "error_description": "Code verifier doesn't match"})
        return give(code["client_id"])

    def info(request, match):
        auth = request.headers.get("authorization", "")
        if not auth.startswith("OAuth ") or auth == "OAuth revoked":
            return Response(401, b"", content_type="text/plain")
        return Response(200, ME)

    def organizations(request, match):
        auth = request.headers.get("authorization", "")
        token = auth[len("OAuth "):] if auth.startswith("OAuth ") else ""
        if not token or token == "revoked" or not (token in data()["tokens"]
                                                     or token.startswith("y0_")):
            return Response(401, {"code": 16, "message": "Unauthenticated"})
        mode = data()["orgs"]
        if mode == "forbidden":
            return Response(403, {"code": 7, "message": "Permission denied"})
        return Response(200, {"organizations": ORGS.get(mode, []), "nextPageToken": ""})

    def control(request, match):
        body = request.json() or {}
        data()["deny"] = bool(body.get("deny"))
        if "refresh" in body:
            data()["refresh_ok"] = bool(body["refresh"])
        if "expires_in" in body:
            data()["expires_in"] = int(body["expires_in"])
        return Response(204)

    def orgs_control(request, match):
        body = request.json() or {}
        data()["orgs"] = body.get("mode", "one")
        return Response(204)

    router.add("GET", r"/authorize", authorize)
    router.add("POST", r"/token", token)
    router.add("GET", r"/info", info)
    router.add("GET", r"/directory/v1/org", organizations)
    router.add("POST", r"/_mock/oauth", control)
    router.add("GET", r"/_mock/oauth/code", lambda request, match: Response(
        200, {"code": data()["last_code"]}))
    router.add("POST", r"/_mock/orgs", orgs_control)
