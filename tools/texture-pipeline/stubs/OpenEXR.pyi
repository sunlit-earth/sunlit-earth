"""Minimal stubs for the parts of the OpenEXR bindings this tool uses.

The wheel ships a bare extension module with no type information, so a type
checker can say nothing about it without these.
"""

from types import TracebackType
from typing import Any, Self

import numpy as np

ZIP_COMPRESSION: int
scanlineimage: int

class Channel:
    pixels: np.ndarray

class File:
    def __init__(
        self,
        *args: Any,
        separate_channels: bool = False,
        header_only: bool = False,
    ) -> None: ...
    def __enter__(self) -> Self: ...
    def __exit__(
        self,
        exc_type: type[BaseException] | None,
        exc_value: BaseException | None,
        traceback: TracebackType | None,
    ) -> None: ...
    def channels(self, part_index: int = 0) -> dict[str, Channel]: ...
    def header(self, part_index: int = 0) -> dict[str, Any]: ...
    def write(self, filename: str) -> None: ...
