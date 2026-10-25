import asyncio, sys
import websockets

async def main(proxy_port, echo_port):
    uri = f'ws://127.0.0.1:{echo_port}/echo'
    async with websockets.connect(uri, proxy=f'http://127.0.0.1:{proxy_port}', compression=None) as ws:
        await ws.send('hello-ws')
        print('text ->', await ws.recv())
        await ws.send(b'\x01\x02\x03\xff')
        print('bin  ->', await ws.recv())
        pong = await ws.ping(b'pp')
        await pong
        print('ping ok')
    print('closed', ws.close_code)

asyncio.run(main(int(sys.argv[1]), int(sys.argv[2]) if len(sys.argv) > 2 else 8896))

