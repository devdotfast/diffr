"""Exercise Rust -> Bun -> NDJSON -> terminal, including real mouse escape sequences."""
import errno
import fcntl
import re
import os
import pty
import select
import signal
import struct
import sys
import termios
import time

binary, bun, before, after = sys.argv[1:]
pid, master = pty.fork()
if pid == 0:
    os.environ['DIFFR_BUN'] = bun
    os.environ['TERM'] = 'xterm-256color'
    os.execv(binary, [binary, '--no-index', *(['--exit-code'] if os.environ.get('DIFFR_TEST_EXIT_CODE') else []), '--', before, after])
fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 24, 100, 0, 0))
output = bytearray()
ANSI = re.compile(rb'\x1b\[[0-9;?]*[A-Za-z]')
def until(token):
    """Wait for token in the screen text; syntax colouring splits words with escape codes."""
    deadline = time.monotonic() + 12
    while time.monotonic() < deadline:
        if token in ANSI.sub(b'', bytes(output)):
            output.clear()
            return
        if select.select([master], [], [], 0.1)[0]:
            try:
                data = os.read(master, 65536)
            except OSError as error:
                if error.errno == errno.EIO:
                    raise AssertionError(f'TUI exited before {token!r}: {bytes(output)[-2000:]!r}')
                raise
            if not data:
                break
            output.extend(data)
    raise AssertionError(f'Missing {token!r}: {bytes(output)[-2000:]!r}')
try:
    until(b'console.log(name);')
    os.write(master, b's')
    until(b'split [s]')
    # Width 100 has a 28-column file sidebar. The header is at x=30,y=2 (1-based).
    os.write(master, b'\x1b[<0;31;2M\x1b[<0;31;2m')
    until('▸'.encode())
    os.write(master, b'\x1b[<0;31;2M\x1b[<0;31;2m')
    until('▾'.encode())
    os.write(master, b'q')
    deadline = time.monotonic() + 5
    while time.monotonic() < deadline:
        if select.select([master], [], [], 0)[0]:
            try: os.read(master, 65536)
            except OSError: pass
        result, status = os.waitpid(pid, os.WNOHANG)
        if result:
            assert os.waitstatus_to_exitcode(status) == (1 if os.environ.get('DIFFR_TEST_EXIT_CODE') else 0), status
            pid = 0
            print('PTY launch, streamed rendering, layout switch, mouse file toggle, and clean exit passed')
            break
        time.sleep(0.05)
    else:
        raise AssertionError('TUI did not exit')
finally:
    if pid:
        os.kill(pid, signal.SIGKILL)
        os.waitpid(pid, 0)
    os.close(master)
