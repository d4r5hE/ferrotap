import asyncio, sys
import websockets

async def echo(ws):
    async for m in ws:
        await ws.send(m)

async def main(port):
    async with websockets.serve(echo, '127.0.0.1', port):
        print('echo listening', port, flush=True)
        await asyncio.Future()

asyncio.run(main(int(sys.argv[1]) if len(sys.argv) > 1 else 8896))
