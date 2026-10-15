"""Yandex ID's OAuth for the fake Yandex (mock_yandex.py): the code with PKCE and who signed in.

- GET /authorize — answers at once as if the user allowed it: 302 to `redirect_uri` with `code` and
  `state` (`POST /_mock/oauth {"deny": true}` — with `error=access_denied` instead).
- POST /token — `grant_type=authorization_code` with `code`, `client_id`, `code_verifier`: the
  verifier's S256 must be the code's challenge; a token that lives a year.
- GET /info — who the token is.

Tokens it gave are in `state.data["oauth"]["tokens"]`; Tracker's part accepts them, and pasted
ones that start with `y0_`.
"""
import base64
import hashlib
import secrets
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


def register(router, state):
    def reset():
        state.data["oauth"] = {"codes": {}, "tokens": set(), "deny": False}

    reset()
    state.resets.append(reset)
    data = lambda: state.data["oauth"]  # noqa: E731

    def authorize(request, match):
        redirect = request.arg("redirect_uri", "")
        if not redirect.startswith("http://127.0.0.1:"):
            return Response(400, {"error": "invalid_request",
                                  "error_description": "redirect_uri isn't registered"})
        if request.arg("code_challenge_method") != "S256" or not request.arg("code_challenge"):
            return Response(400, {"error": "invalid_request",
                                  "error_description": "PKCE S256 is expected"})
        state_ = request.arg("state", "")
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

    def token(request, match):
        form = request.form()
        if form.get("grant_type") != "authorization_code":
            return Response(400, {"error": "unsupported_grant_type"})
        code = data()["codes"].pop(form.get("code", ""), None)
        if not code:
            return Response(400, {"error": "invalid_grant",
                                  "error_description": "Code has expired"})
        if code["client_id"] != form.get("client_id"):
            return Response(400, {"error": "invalid_client"})
        if challenge(form.get("code_verifier", "")) != code["challenge"]:
            return Response(400, {"error": "invalid_grant",
                                  "error_description": "Code verifier doesn't match"})
        access = "y0_mock_" + secrets.token_hex(6)
        data()["tokens"].add(access)
        return Response(200, {"access_token": access, "refresh_token": "1:mock:" + access,
                              "expires_in": 31536000, "token_type": "bearer"})

    def info(request, match):
        auth = request.headers.get("authorization", "")
        if not auth.startswith("OAuth ") or auth == "OAuth revoked":
            return Response(401, b"", content_type="text/plain")
        return Response(200, ME)

    def control(request, match):
        body = request.json() or {}
        data()["deny"] = bool(body.get("deny"))
        return Response(204)

    router.add("GET", r"/authorize", authorize)
    router.add("POST", r"/token", token)
    router.add("GET", r"/info", info)
    router.add("POST", r"/_mock/oauth", control)
