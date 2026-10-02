"""Exception hierarchy.

Every error raised by the native core is an :class:`OpenReadoutError` and also an instance of
the closest built-in exception, so ordinary ``except OSError`` / ``except ValueError`` code keeps
working. Each carries the same ``code``, ``exit_code`` and ``hint`` the CLI reports in its JSON
error envelope (see https://openreadout.github.io/openreadout/reference/commands.html#exit-codes).
"""

from __future__ import annotations

__all__ = [
    "CorruptFileError",
    "InstrumentFileNotFoundError",
    "InstrumentIOError",
    "OpenReadoutError",
    "UnknownFormatError",
    "UnsupportedFeatureError",
    "UsageError",
]


class OpenReadoutError(Exception):
    """Base class for every error raised by :mod:`openreadout`.

    Attributes:
        code: Stable machine-readable code (``"io"``, ``"usage"``, ``"corrupt_file"``, ...).
        exit_code: The exit code the CLI uses for the same failure.
        hint: A suggestion a human or agent can act on, or ``None``.
    """

    code: str = "error"
    exit_code: int = 1
    hint: str | None = None


class UsageError(OpenReadoutError, ValueError):
    """Bad arguments or an impossible request (e.g. a plane index out of range)."""

    code = "usage"
    exit_code = 2


class UnknownFormatError(OpenReadoutError, ValueError):
    """The file does not match any supported format's signature."""

    code = "unknown_format"
    exit_code = 3


class CorruptFileError(OpenReadoutError, RuntimeError):
    """The file violates its own format's invariants (truncation, bad offsets, ...)."""

    code = "corrupt_file"
    exit_code = 4


class InstrumentIOError(OpenReadoutError, OSError):
    """Operating-system I/O failure (permissions, disk errors, ...)."""

    code = "io"
    exit_code = 5


class InstrumentFileNotFoundError(InstrumentIOError, FileNotFoundError):
    """The path does not exist."""


class UnsupportedFeatureError(OpenReadoutError, NotImplementedError):
    """The format is known but this file uses a feature that is not decoded yet."""

    code = "unsupported_feature"
    exit_code = 6
