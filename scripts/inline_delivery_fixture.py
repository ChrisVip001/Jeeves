"""Loopback shops, model responses and a receiving connector for the main-chat smoke."""
import datetime
import json
import sys
from pathlib import Path
from urllib.request import urlopen

OFFERS = {
    'a': {'model': 'A', 'spec': '256 GB', 'currency': 'CNY', 'price': 1690, 'shipping': 30, 'noise': '42 dB'},
    'b': {'model': 'B', 'spec': '256 GB', 'currency': 'CNY', 'price': 1710, 'shipping': None, 'noise': '28 dB'},
}


def offer(path):
    return OFFERS.get(path.removeprefix('/shops/')) if path.startswith('/shops/') else None


def configure(home, origin):
    config = {'servers': [{
        'name': 'shop', 'command': sys.executable, 'args': [__file__, str(home), origin],
        'manifest': {'category': 'other', 'tools': {
            'quote': {'effect': 'read'}, 'deliver': {'effect': 'send'},
        }, 'verbs': {}},
    }]}
    Path(home, 'mcp.json').write_text(json.dumps(config))


def serve(home, origin):
    for line in sys.stdin:
        message = json.loads(line)
        if 'id' not in message:
            continue
        method = message['method']
        if method == 'initialize':
            result = {'protocolVersion': '2025-06-18', 'capabilities': {'tools': {}},
                      'serverInfo': {'name': 'shop fixture', 'version': '1'}}
        elif method == 'tools/list':
            result = {'tools': [
                {'name': name, 'description': description, 'inputSchema': {
                    'type': 'object', 'required': keys, 'properties': {key: {'type': 'string'} for key in keys}}}
                for name, description, keys in [('quote', 'Read a shop offer', ['shop']),
                                               ('deliver', 'Deliver the comparison', ['destination', 'body'])]]}
        elif method == 'tools/call':
            params = message['params']
            if params['name'] == 'quote':
                key = params['arguments']['shop']
                assert key in OFFERS
                url = origin + '/shops/' + key
                with urlopen(url, timeout=5) as response:
                    quote = json.load(response)
                at = datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds')
                text = json.dumps(dict(quote, url=url, checked=at))
            else:
                assert params['name'] == 'deliver'
                with Path(home, 'receiver.jsonl').open('a') as receiver:
                    receiver.write(json.dumps(params['arguments']) + '\n')
                text = 'Error: 字样测试 / 已收录 #R1'
            result = {'content': [{'type': 'text', 'text': text}], 'isError': False}
        else:
            result = {}
        print(json.dumps({'jsonrpc': '2.0', 'id': message['id'], 'result': result}), flush=True)


def respond(provider, messages, tools):
    user = next(m['content'] for m in reversed(messages) if m['role'] == 'user')
    query = user if isinstance(user, str) else ' '.join(b.get('text', '') for b in user)
    query = query.rsplit('Current human task:', 1)[-1].strip()
    last = messages[-1]
    if query == '这两款主要在家用，哪个更安静？':
        images = [b['image_url']['url'] for b in user if b['type'] == 'image_url']
        assert len(set(images)) == 2 and all(url.startswith('data:image/png;base64,') for url in images)
        provider.chunk({'content': '两张截图里 B 的 28 dB 比 A 的 42 dB 安静，家用选 B。'}, 'stop')
    elif query == '再查一下同规格到手价，算上运费。':
        if last['role'] == 'tool':
            quotes = [json.loads(m['content']) for m in messages[-2:]]
            lines = []
            for q in quotes:
                total = '运费待核实' if q['shipping'] is None else str(q['price'] + q['shipping'])
                lines.append(f"[{q['model']}]({q['url']}) {q['spec']}，标价 {q['price']}，到手 {total}，查询时间 {q['checked']}")
            provider.chunk({'content': '\n\n'.join(lines)}, 'stop')
        else:
            calls(provider, tools, [('shop_quote', {'shop': key}) for key in OFFERS])
    elif query == '第二款帮我盯三天，低于1800再告诉我，先别买。':
        if last['role'] == 'tool':
            provider.chunk({'content': last['content']}, 'stop')
        else:
            comparison = next(m['content'] for m in reversed(messages) if m['role'] == 'assistant'
                              and '运费待核实' in (m.get('content') or ''))
            url = comparison.split('[B](')[1].split(')')[0]
            calls(provider, tools, [('task', {'action': 'create', 'items': [{
                'title': '三天盯价', 'instruction': query,
                'price_watch': {'model': 'B', 'spec': '256 GB', 'url': url, 'days': 3, 'below': 1800}
            }]})])
    elif query == '改成低于1700才提醒，其他照旧。':
        if last['role'] == 'tool':
            provider.chunk({'content': last['content']}, 'stop')
        else:
            tool = next(t['function'] for t in tools if t['function']['name'] == 'selected_price_watch')
            task = tool['parameters']['properties']['task_id']['const']
            calls(provider, tools, [('selected_price_watch', {'action':'amend_price', 'task_id':task, 'below':1700})])
    elif query.startswith('把比价发送到 '):
        destination = query.split('把比价发送到')[-1].strip()
        if last['role'] == 'tool':
            content = last['content']
            if '已收录 #R1' in content:
                reply = content
            elif 'denied' in content.lower():
                reply = '已拒绝，本次没有发送。'
            else:
                raise AssertionError(content)
            provider.chunk({'content': reply}, 'stop')
        else:
            comparison = next(m['content'] for m in reversed(messages) if m['role'] == 'assistant'
                              and '运费待核实' in (m.get('content') or ''))
            calls(provider, tools, [('shop_deliver', {'destination': destination, 'body': comparison})])
    else:
        return False
    return True


def calls(provider, tools, requested):
    available = {tool['function']['name'] for tool in tools}
    assert all(name in available for name, _ in requested), available
    provider.chunk({'tool_calls': [
        {'index': i, 'id': f'{name}-{i}', 'type': 'function',
         'function': {'name': name, 'arguments': json.dumps(args)}}
        for i, (name, args) in enumerate(requested)]})
    provider.chunk({}, 'tool_calls')


if __name__ == '__main__':
    serve(Path(sys.argv[1]), sys.argv[2])
