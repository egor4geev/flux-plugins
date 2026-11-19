#!/usr/bin/env python3
"""A fake Telegram Bot API for Flux Remote's tests and scenarios (no network, no real bot).

    fake_telegram.py serve [--port 8790] [--token 7000000001:FAKE-token] [--latency-ms 0]
    fake_telegram.py ctl  [--port 8790] <path> [json]       # POST (GET for /_fake/state…)
    fake_telegram.py wait [--port 8790] --text "Готово" [--thread N] [--timeout 30]

Flux sends the plugin's requests here with `FLUX_PLUGIN_HTTP_OVERRIDE=https://api.telegram.org=
http://127.0.0.1:<port>`. The bot is @flux_fake_bot; the owner (id 5100000001, «Егор») and a
stranger (5100000002) write to it through the control API below. It behaves as the live bot did
(wiki «Telegram APIs» → «Живая проверка»): topics in the private chat; a message outside topics opens
a topic with an implicit name (its first 16 characters) — `forum_topic_created` from the user, then
the message with `reply_to_message`; the bot's own topics come back as `forum_topic_created` from
the bot; a message to a topic ends its draft; «Stop» comes as `stopped_message_generation` with the
draft's id as a string; a deleted topic: `TOPIC_ID_INVALID`; an edit that changes nothing: «message is
not modified»; a second `getUpdates` while one waits ends the first with 409.

Control (JSON POST unless said):
  /_fake/reset                     {"topics_enabled": true}
  /_fake/config                    {"topics_enabled", "conflict", "latency_ms",
                                    "limit": {"method": "sendMessage", "retry_after": 3, "times": 1}}
  /_fake/user/message              {"text", "thread": N | null (a new chat), "user": "owner"|"stranger"}
  /_fake/user/start                {"code"}       — the binding link's «Start»: `/start <code>`
  /_fake/user/press                {"button": "Разрешить" (part of a button's text), "thread": N}
                                   or {"message_id": M, "data": "r:1:a"}
  /_fake/user/stop                 {"thread": N} or {"draft_id": D}; "no_thread": true drops the topic
  /_fake/user/delete_topic         {"thread": N}
  GET /_fake/state                 topics, messages (with buttons), drafts, answers, the last calls
  GET /_fake/transcript?thread=N   the topic as lines: "bot: … [Allow | Deny]"
"""
import argparse
import json
import random
import sys
import threading
import time
import urllib.parse
import urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

BOT = {'id': 7000000001, 'is_bot': True, 'first_name': 'Flux Fake', 'username': 'flux_fake_bot'}
USERS = {
    'owner': {'id': 5100000001, 'is_bot': False, 'first_name': 'Егор', 'username': 'owner', 'language_code': 'ru'},
    'stranger': {'id': 5100000002, 'is_bot': False, 'first_name': 'Чужой', 'username': 'stranger'},
}


class World:
    def __init__(self, token, latency_ms):
        self.token = token
        self.lock = threading.Condition()
        self.latency_ms = latency_ms
        self.reset(True)

    def reset(self, topics_enabled=True):
        self.topics_enabled = topics_enabled
        self.conflict = False
        self.limits = {}
        self.updates = []
        self.next_update = 1
        self.next_message = 1
        self.next_thread = 1000
        self.topics = {}
        self.messages = {}
        self.drafts = {}
        self.answers = []
        self.queries = {}
        self.calls = []
        self.poll_generation = 0
        self.polls = 0

    # --- helpers (under the lock) ---
    def chat(self, user):
        return {'id': user['id'], 'first_name': user['first_name'], 'username': user.get('username'), 'type': 'private'}

    def push(self, kind, value):
        self.updates.append({'update_id': self.next_update, kind: value})
        self.next_update += 1
        self.lock.notify_all()

    def message_object(self, m):
        out = {'message_id': m['message_id'], 'from': BOT if m['from'] == 'bot' else m['user'],
               'chat': {'id': m['chat'], 'type': 'private'}, 'date': int(m['date'])}
        if m['thread']:
            out['message_thread_id'] = m['thread']
            out['is_topic_message'] = True
        if m.get('text') is not None:
            out['text'] = m['text']
        if m.get('keyboard'):
            out['reply_markup'] = {'inline_keyboard': m['keyboard']}
        if m.get('service'):
            out.update(m['service'])
        return out

    def add_message(self, chat, thread, sender, text=None, keyboard=None, kind='message', user=None, service=None):
        m = {'message_id': self.next_message, 'chat': chat, 'thread': thread, 'from': sender, 'user': user,
             'text': text, 'keyboard': keyboard or [], 'kind': kind, 'edits': 0, 'deleted': False,
             'date': time.time(), 'service': service}
        self.next_message += 1
        self.messages[m['message_id']] = m
        return m

    def topic_ok(self, thread):
        return thread is None or (thread in self.topics and not self.topics[thread]['deleted'])

    def end_draft(self, chat, thread):
        for key, draft in self.drafts.items():
            if key[0] == chat and key[1] == thread and draft['state'] == 'live':
                draft['state'] = 'ended'

    def new_topic(self, chat, name, color, user=None, implicit=False):
        thread = self.next_thread
        self.next_thread += 1
        self.topics[thread] = {'thread': thread, 'chat': chat, 'name': name, 'icon_color': color,
                               'deleted': False, 'created_by': 'user' if user else 'bot'}
        created = {'name': name, 'icon_color': color}
        if implicit:
            created['is_name_implicit'] = True
        service = self.add_message(chat, thread, 'user' if user else 'bot', user=user,
                                   service={'forum_topic_created': created})
        self.push('message', self.message_object(service))
        return thread, service

    # --- the Bot API ---
    def api(self, method, p):
        known = {u['id'] for u in USERS.values()}
        chat = p.get('chat_id')
        thread = p.get('message_thread_id')
        if method == 'getMe':
            return dict(BOT, has_topics_enabled=self.topics_enabled, allows_users_to_create_topics=True)
        if method in ('sendMessage', 'sendRichMessage', 'sendMessageDraft', 'editMessageText',
                      'editMessageReplyMarkup', 'deleteMessage', 'createForumTopic', 'editForumTopic',
                      'deleteForumTopic', 'pinChatMessage', 'unpinAllChatMessages', 'sendChatAction',
                      'sendDocument', 'sendPhoto') and chat not in known:
            raise Refused(400, 'Bad Request: chat not found')
        if method in ('sendMessage', 'sendRichMessage', 'sendDocument', 'sendPhoto'):
            if not self.topic_ok(thread):
                raise Refused(400, 'Bad Request: message thread not found')
            if method == 'sendRichMessage':
                text = (p.get('rich_message') or {}).get('markdown')
                if not text:
                    raise Refused(400, 'Bad Request: rich message is empty')
                if len(text) > 32768:
                    raise Refused(400, 'Bad Request: rich message is too long')
            elif method == 'sendMessage':
                text = p.get('text') or ''
                if not text.strip():
                    raise Refused(400, 'Bad Request: message text is empty')
                if len(text) > 4096:
                    raise Refused(400, 'Bad Request: message is too long')
            else:
                text = '[%s]' % method[4:].lower()
            keyboard = (p.get('reply_markup') or {}).get('inline_keyboard')
            self.end_draft(chat, thread)
            m = self.add_message(chat, thread, 'bot', text, keyboard, kind={'sendRichMessage': 'rich'}.get(method, 'message'))
            return self.message_object(m)
        if method == 'sendMessageDraft':
            draft = p.get('draft_id')
            if not isinstance(draft, int) or draft == 0:
                raise Refused(400, 'Bad Request: draft_id must be a non-zero integer')
            text = p.get('text') or ''
            if len(text) > 4096:
                raise Refused(400, 'Bad Request: text is too long')
            key = (chat, thread, draft)
            d = self.drafts.setdefault(key, {'chat': chat, 'thread': thread, 'draft_id': draft, 'text': '',
                                             'updates': 0, 'state': 'live', 'can_stop': False})
            if d['state'] == 'live':
                d.update(text=text, can_stop=bool(p.get('can_stop')), updated=time.time())
                d['updates'] += 1
            return True
        if method in ('editMessageText', 'editMessageReplyMarkup'):
            m = self.messages.get(p.get('message_id'))
            if not m or m['deleted'] or m['chat'] != chat or m['from'] != 'bot':
                raise Refused(400, 'Bad Request: message to edit not found')
            text = p.get('text', m['text']) if method == 'editMessageText' else m['text']
            # Without `reply_markup` both drop the buttons, as Telegram does.
            keyboard = (p.get('reply_markup') or {}).get('inline_keyboard') or []
            if text == m['text'] and keyboard == m['keyboard']:
                raise Refused(400, 'Bad Request: message is not modified: specified new message content and reply markup are exactly the same as a current content and reply markup of the message')
            m.update(text=text, keyboard=keyboard)
            m['edits'] += 1
            return self.message_object(m)
        if method == 'deleteMessage':
            m = self.messages.get(p.get('message_id'))
            if not m or m['deleted']:
                raise Refused(400, 'Bad Request: message to delete not found')
            m['deleted'] = True
            return True
        if method == 'answerCallbackQuery':
            query = self.queries.get(p.get('callback_query_id'))
            if not query or time.time() - query['at'] > 15:
                raise Refused(400, 'Bad Request: query is too old and response timeout expired or query ID is invalid')
            self.answers.append({'query': p.get('callback_query_id'), 'text': p.get('text'), 'data': query['data']})
            return True
        if method == 'createForumTopic':
            if not self.topics_enabled:
                raise Refused(400, 'Bad Request: the chat is not a forum')
            name = (p.get('name') or '').strip()
            if not 1 <= len(name) <= 128:
                raise Refused(400, 'Bad Request: TOPIC_NAME_INVALID')
            new, _ = self.new_topic(chat, name, p.get('icon_color', 7322096))
            return {'message_thread_id': new, 'name': name, 'icon_color': p.get('icon_color', 7322096)}
        if method == 'editForumTopic':
            if thread not in self.topics or self.topics[thread]['deleted']:
                raise Refused(400, 'Bad Request: TOPIC_ID_INVALID')
            if 'name' in p:
                self.topics[thread]['name'] = p['name']
            return True
        if method == 'deleteForumTopic':
            if thread not in self.topics or self.topics[thread]['deleted']:
                raise Refused(400, 'Bad Request: TOPIC_ID_INVALID')
            self.topics[thread]['deleted'] = True
            return True
        if method in ('setMyCommands', 'deleteMyCommands', 'pinChatMessage', 'unpinAllChatMessages', 'sendChatAction'):
            return True
        raise Refused(404, 'Not Found: method not found')

    # --- the user ---
    def user_message(self, text, thread=None, who='owner'):
        user = USERS[who]
        chat = user['id']
        if thread is None:
            thread, service = self.new_topic(chat, text[:16] or '…', 7322096, user=user, implicit=True)
        else:
            if not self.topic_ok(thread):
                raise Refused(400, 'no such topic')
            service = None
        m = self.add_message(chat, thread, 'user', text, user=user)
        obj = self.message_object(m)
        if service:
            obj['reply_to_message'] = self.message_object(service)
        self.push('message', obj)
        return {'thread': thread, 'message_id': m['message_id']}

    def press(self, body):
        who = body.get('user', 'owner')
        user = USERS[who]
        target, data = None, body.get('data')
        if body.get('message_id'):
            target = self.messages.get(body['message_id'])
        else:
            for m in sorted(self.messages.values(), key=lambda m: -m['message_id']):
                if m['from'] != 'bot' or m['deleted'] or (body.get('thread') is not None and m['thread'] != body['thread']):
                    continue
                for row in m['keyboard']:
                    for b in row:
                        if body.get('button', '') in b.get('text', '') and 'callback_data' in b:
                            target, data = m, b['callback_data']
                            break
                    if target:
                        break
                if target:
                    break
        if not target or not data:
            raise Refused(400, 'no such button')
        query = str(random.randint(10**17, 10**18))
        self.queries[query] = {'at': time.time(), 'data': data}
        self.push('callback_query', {'id': query, 'from': user, 'chat_instance': '1',
                                     'message': self.message_object(target), 'data': data})
        return {'query': query, 'data': data, 'message_id': target['message_id']}

    def stop(self, body):
        live = [d for d in self.drafts.values() if d['state'] == 'live' and
                (body.get('draft_id') in (None, d['draft_id'])) and
                (body.get('thread') is None or d['thread'] == body['thread'])]
        if not live:
            raise Refused(400, 'no live draft')
        d = max(live, key=lambda d: d.get('updated', 0))
        if not d['can_stop']:
            raise Refused(400, 'the draft has no Stop')
        d['state'] = 'stopped'
        stopped = {'chat': {'id': d['chat'], 'type': 'private'}, 'draft_id': str(d['draft_id'])}
        if d['thread'] and not body.get('no_thread'):
            stopped['message_thread_id'] = d['thread']
        self.push('stopped_message_generation', stopped)
        return {'draft_id': d['draft_id'], 'text': d['text']}

    def state(self):
        def buttons(m):
            return [[b.get('text') for b in row] for row in m['keyboard']]
        return {
            'bot': BOT, 'owner': USERS['owner'], 'topics_enabled': self.topics_enabled,
            'topics': list(self.topics.values()),
            'messages': [{'message_id': m['message_id'], 'thread': m['thread'], 'from': m['from'], 'kind': m['kind'],
                          'text': m['text'], 'buttons': buttons(m), 'data': [[b.get('callback_data') for b in row] for row in m['keyboard']],
                          'edits': m['edits'], 'deleted': m['deleted'], 'service': m['service']}
                         for m in self.messages.values()],
            'drafts': [{k: v for k, v in d.items() if k != 'chat'} for d in self.drafts.values()],
            'answers': self.answers, 'calls': self.calls[-300:], 'pending_updates': len(self.updates),
            'polls': self.polls,
        }

    def transcript(self, thread):
        lines = []
        for m in sorted(self.messages.values(), key=lambda m: m['message_id']):
            if m['thread'] != thread or m['service']:
                continue
            text = (m['text'] or '').replace('\n', ' ⏎ ')
            keys = ' | '.join(b for row in m['keyboard'] for b in [x.get('text', '') for x in row])
            lines.append(f"{m['from']}{' (rich)' if m['kind'] == 'rich' else ''}{' (deleted)' if m['deleted'] else ''}: {text}"
                         + (f' [{keys}]' if keys else ''))
        for d in self.drafts.values():
            if d['thread'] == thread and d['state'] == 'live':
                lines.append(f"draft {d['draft_id']} ({d['updates']} updates): {d['text']}")
        return lines


class Refused(Exception):
    def __init__(self, code, description, retry_after=None):
        super().__init__(description)
        self.code, self.description, self.retry_after = code, description, retry_after


def handler_for(world):
    class Handler(BaseHTTPRequestHandler):
        def log_message(self, format, *args):
            pass

        def reply(self, code, payload):
            body = json.dumps(payload, ensure_ascii=False).encode()
            self.send_response(code)
            self.send_header('Content-Type', 'application/json')
            self.send_header('Content-Length', str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def body(self):
            length = int(self.headers.get('Content-Length') or 0)
            raw = self.rfile.read(length) if length else b''
            if 'multipart/form-data' in (self.headers.get('Content-Type') or ''):
                return {'_multipart': len(raw)}
            try:
                return json.loads(raw or b'{}')
            except ValueError:
                return dict(urllib.parse.parse_qsl(raw.decode(errors='replace')))

        def do_GET(self):
            url = urllib.parse.urlparse(self.path)
            if url.path == '/_fake/state':
                with world.lock:
                    return self.reply(200, world.state())
            if url.path == '/_fake/transcript':
                thread = urllib.parse.parse_qs(url.query).get('thread', [None])[0]
                with world.lock:
                    return self.reply(200, world.transcript(int(thread) if thread else None))
            return self.do_POST()

        def do_POST(self):
            url = urllib.parse.urlparse(self.path)
            body = self.body()
            parts = url.path.strip('/').split('/')
            try:
                if parts[0] == '_fake':
                    return self.control('/'.join(parts[1:]), body)
                if len(parts) != 2 or not parts[0].startswith('bot'):
                    raise Refused(404, 'Not Found')
                if parts[0][3:] != world.token:
                    raise Refused(401, 'Unauthorized')
                method = parts[1]
                if method == 'getUpdates':
                    return self.reply(200, {'ok': True, 'result': self.get_updates(body)})
                if world.latency_ms:
                    time.sleep(world.latency_ms / 1000)
                with world.lock:
                    world.calls.append({'t': round(time.time(), 3), 'method': method, 'params': body})
                    limit = world.limits.get(method)
                    if limit and limit['times'] > 0:
                        limit['times'] -= 1
                        raise Refused(429, f"Too Many Requests: retry after {limit['retry_after']}", limit['retry_after'])
                    result = world.api(method, body)
                return self.reply(200, {'ok': True, 'result': result})
            except Refused as refused:
                payload = {'ok': False, 'error_code': refused.code, 'description': refused.description}
                if refused.retry_after:
                    payload['parameters'] = {'retry_after': refused.retry_after}
                return self.reply(refused.code, payload)

        def get_updates(self, body):
            offset = body.get('offset')
            timeout = min(float(body.get('timeout') or 0), 50)
            with world.lock:
                if world.conflict:
                    raise Refused(409, 'Conflict: terminated by other getUpdates request; make sure that only one bot instance is running')
                world.polls += 1
                world.poll_generation += 1
                mine = world.poll_generation
                world.lock.notify_all()
                if offset is not None:
                    world.updates = [u for u in world.updates if u['update_id'] >= int(offset)]
                end = time.time() + timeout
                while True:
                    if world.poll_generation != mine:
                        raise Refused(409, 'Conflict: terminated by other getUpdates request; make sure that only one bot instance is running')
                    if world.updates or time.time() >= end:
                        return list(world.updates)
                    world.lock.wait(max(0.01, min(1.0, end - time.time())))

        def control(self, path, body):
            with world.lock:
                try:
                    if path == 'reset':
                        world.reset(body.get('topics_enabled', True))
                        result = {}
                    elif path == 'config':
                        for key in ('topics_enabled', 'conflict', 'latency_ms'):
                            if key in body:
                                setattr(world, key, body[key])
                        if body.get('limit'):
                            world.limits[body['limit']['method']] = dict(body['limit'])
                        world.lock.notify_all()
                        result = {}
                    elif path == 'user/message':
                        result = world.user_message(body.get('text', ''), body.get('thread'), body.get('user', 'owner'))
                    elif path == 'user/start':
                        result = world.user_message(f"/start {body.get('code', '')}".strip(), None, body.get('user', 'owner'))
                    elif path == 'user/press':
                        result = world.press(body)
                    elif path == 'user/stop':
                        result = world.stop(body)
                    elif path == 'user/delete_topic':
                        world.topics[int(body['thread'])]['deleted'] = True
                        result = {}
                    elif path == 'state':
                        result = world.state()
                    else:
                        raise Refused(404, 'no such control')
                except Refused as refused:
                    return self.reply(400, {'ok': False, 'error': refused.description})
            return self.reply(200, {'ok': True, 'result': result})
    return Handler


def request(port, path, payload=None):
    data = None if payload is None else json.dumps(payload).encode()
    req = urllib.request.Request(f'http://127.0.0.1:{port}{path}', data=data,
                                 headers={'Content-Type': 'application/json'})
    with urllib.request.urlopen(req, timeout=60) as response:
        return json.load(response)


def main():
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest='command', required=True)
    serve = sub.add_parser('serve')
    serve.add_argument('--port', type=int, default=8790)
    serve.add_argument('--token', default='7000000001:FAKE-token')
    serve.add_argument('--latency-ms', type=int, default=0)
    ctl = sub.add_parser('ctl')
    ctl.add_argument('--port', type=int, default=8790)
    ctl.add_argument('path')
    ctl.add_argument('json', nargs='?')
    wait = sub.add_parser('wait')
    wait.add_argument('--port', type=int, default=8790)
    wait.add_argument('--text', required=True)
    wait.add_argument('--thread', type=int)
    wait.add_argument('--timeout', type=float, default=30)
    args = parser.parse_args()
    if args.command == 'serve':
        world = World(args.token, args.latency_ms)
        server = ThreadingHTTPServer(('127.0.0.1', args.port), handler_for(world))
        server.daemon_threads = True
        print(f'fake Telegram on http://127.0.0.1:{args.port}', flush=True)
        server.serve_forever()
    elif args.command == 'ctl':
        path = args.path if args.path.startswith('/') else '/_fake/' + args.path
        payload = json.loads(args.json) if args.json else ({} if not path.startswith('/_fake/state') and 'transcript' not in path else None)
        print(json.dumps(request(args.port, path, payload), ensure_ascii=False, indent=1))
    else:
        end = time.time() + args.timeout
        while time.time() < end:
            state = request(args.port, '/_fake/state')
            for m in state['messages']:
                if m['from'] == 'bot' and args.text in (m['text'] or '') and (args.thread is None or m['thread'] == args.thread):
                    print(json.dumps(m, ensure_ascii=False))
                    return
            for d in state['drafts']:
                if args.text in d['text'] and (args.thread is None or d['thread'] == args.thread):
                    print(json.dumps(d, ensure_ascii=False))
                    return
            time.sleep(0.3)
        print(f'timed out waiting for {args.text!r}', file=sys.stderr)
        sys.exit(1)


if __name__ == '__main__':
    main()
