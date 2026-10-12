"""Worker threads for the steps that spend their time in numpy and scipy.

Those libraries release the GIL, so bands of rows or independent fields run at
once on threads without copying anything. Nothing here changes a result: a
step that uses :func:`pmap` returns what it returned serially, in the same
order. The count comes from, in turn, the caller, :func:`configure`, the
``TEXTURE_PIPELINE_WORKERS`` environment variable, and the machine's logical
cores up to :data:`LIMIT`; ``1`` runs everything on the calling thread.
"""

import os
from collections.abc import Callable, Iterable
from concurrent.futures import ThreadPoolExecutor

ENV = "TEXTURE_PIPELINE_WORKERS"
"""The environment variable that sets the worker count; blank is unset."""

LIMIT = 16
"""The most workers taken from the machine's cores without being asked."""


class _Setting:
    value: int | None = None


_setting = _Setting()


def configure(count: int | None) -> None:
    """Set the worker count for the rest of the process.

    :param count: The count, or ``None`` to go back to the environment and the
        machine.
    :raises ValueError: If the count is below 1.
    """
    if count is not None and count < 1:
        msg = f"The worker count is at least 1, not {count}."
        raise ValueError(msg)
    _setting.value = count


def workers(requested: int | None = None) -> int:
    """Return the number of threads a parallel step may use.

    :param requested: A count the caller asks for; it wins over every setting.
    :returns: At least 1.
    :raises ValueError: If the environment variable is not a whole number.
    """
    if requested is not None:
        return max(1, requested)
    if _setting.value is not None:
        return _setting.value
    text = os.environ.get(ENV, "").strip()
    if text:
        try:
            return max(1, int(text))
        except ValueError:
            msg = f"{ENV} is a whole number of threads, not {text!r}."
            raise ValueError(msg) from None
    return max(1, min(LIMIT, os.cpu_count() or 1))


def pmap[T, R](
    function: Callable[[T], R], items: Iterable[T], count: int | None = None
) -> list[R]:
    """Apply a function to every item on threads, results in the items' order.

    :param function: Called once per item.
    :param items: The items.
    :param count: The workers; :func:`workers` decides when ``None``.
    :returns: The results, in order. The first exception raised is raised.
    """
    todo = list(items)
    threads = min(workers(count), len(todo))
    if threads <= 1:
        return [function(item) for item in todo]
    with ThreadPoolExecutor(threads) as pool:
        return list(pool.map(function, todo))
