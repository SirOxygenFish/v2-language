"""A pool worker that prints a lot (tests/engines/py/fork_pool.v2)."""


def shout(k):
    print(str(k) * 20000)
    return k * 2
