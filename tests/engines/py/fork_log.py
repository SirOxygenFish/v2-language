"""A pool worker that logs (tests/engines/py/fork_logging.v2)."""
import logging

log = logging.getLogger("fork_log")


def work(i):
    log.warning("child logs %d", i)
    return i * 10
