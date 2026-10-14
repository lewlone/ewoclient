"""One GPU verification at a time per machine.

`tools/gates.py` and `tools/render_check.py` drive the real Vulkan device, and
`render_check.py` also opens a window and a local server. The owner's
self-hosted CI runner executes them on the same desktop that local runs (and
agent worktrees) use, so two sweeps could overlap: a 2026-10-14 CI run read
65/66 render-check witnesses while another worktree's render check was running
on the same machine, and a gate sweep died with exit 127 under the same
contention. Neither was a defect in the code under test.

`machine_lock()` takes an exclusive lock on one file in the system temp
directory and holds it for the duration of the `with` block. A second caller
waits (printing who holds it) instead of colliding; the OS drops the lock when
the holder exits, even on a crash, so a stale file never blocks anyone.
"""
from __future__ import annotations

import contextlib
import os
import sys
import tempfile
import time

LOCK_PATH = os.path.join(tempfile.gettempdir(), "rewo-gpu-verify.lock")


def _try_lock(fh) -> bool:
    if os.name == "nt":
        import msvcrt

        # msvcrt locks from the current position: always byte 0, or two
        # processes appending different text would lock different bytes.
        fh.seek(0)
        try:
            msvcrt.locking(fh.fileno(), msvcrt.LK_NBLCK, 1)
            return True
        except OSError:
            return False
    import fcntl

    try:
        fcntl.flock(fh.fileno(), fcntl.LOCK_EX | fcntl.LOCK_NB)
        return True
    except OSError:
        return False


@contextlib.contextmanager
def machine_lock(what: str, timeout_s: float = 3 * 60 * 60):
    """Hold the machine-wide GPU-verification lock while the block runs."""
    fh = open(LOCK_PATH, "a+b")
    try:
        start = time.monotonic()
        announced = False
        while not _try_lock(fh):
            if not announced:
                holder = "another run"
                try:  # Windows refuses reads of a locked byte; best effort
                    fh.seek(1)
                    holder = fh.read(200).decode("utf-8", "replace").strip() or holder
                except OSError:
                    pass
                print(f"[gpu-lock] waiting: {holder} holds {LOCK_PATH}",
                      file=sys.stderr, flush=True)
                announced = True
            if time.monotonic() - start > timeout_s:
                sys.exit(f"[gpu-lock] gave up after {timeout_s:.0f}s waiting for {LOCK_PATH}")
            time.sleep(2)
        # Byte 0 is the lock region; the holder description after it is
        # informational.
        fh.seek(0)
        fh.truncate()
        fh.write(f" {what} (pid {os.getpid()})".encode())
        fh.flush()
        if announced:
            print(f"[gpu-lock] acquired after {time.monotonic() - start:.0f}s",
                  file=sys.stderr, flush=True)
        yield
    finally:
        fh.close()  # releases the lock
