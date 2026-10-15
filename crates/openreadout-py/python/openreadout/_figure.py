"""Pictures: :class:`Preview` (``openreadout preview``) and :class:`Figure` (``openreadout
plot``). Both show themselves in Jupyter and save to a file through a verified write."""

from __future__ import annotations

import json
import os
import tempfile
from typing import Any, Dict, List, Mapping, Optional, Tuple, Union

from . import _native
from ._errors import UsageError

__all__ = ["Figure", "Preview", "plot"]

PathLike = Union[str, "os.PathLike[str]"]


def _write_verified(path: PathLike, data: bytes, overwrite: bool) -> str:
    """Write ``data`` to a temporary file beside ``path``, read it back, compare, then rename
    it into place."""
    out = os.fspath(path)
    if os.path.exists(out) and not overwrite:
        raise UsageError(f"{out} exists; pass overwrite=True to replace it")
    folder = os.path.dirname(os.path.abspath(out))
    fd, tmp = tempfile.mkstemp(prefix=".openreadout-", dir=folder)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        with open(tmp, "rb") as f:
            if f.read() != data:
                raise OSError(f"read-back of {out} differs from what was written")
        os.replace(tmp, out)
    except BaseException:
        if os.path.exists(tmp):
            os.unlink(tmp)
        raise
    return out


def _jsonable(v: Any) -> Any:
    if isinstance(v, os.PathLike):
        return os.fspath(v)
    if isinstance(v, tuple):
        return list(v)
    raise TypeError(f"{type(v).__name__} is not JSON serializable")


class Preview:
    """A preview picture of a file (``openreadout preview``): the encoded bytes and what was
    drawn.

    In Jupyter it shows itself. ``meta`` is the ``preview --json`` data: ``width`` and
    ``height``, the image ``source`` mapping of image previews (picture pixel to file pixel),
    ``panels`` (each plot panel's place and data range), ``marks`` and ``annotations`` (where
    each one landed), ``notes``.
    """

    def __init__(self, meta: Dict[str, Any], data: bytes) -> None:
        self.meta: Dict[str, Any] = meta
        self.data: bytes = data

    @property
    def format(self) -> str:
        """``"png"``, ``"jpeg"`` or ``"svg"``."""
        return str(self.meta.get("encoding", "png"))

    @property
    def width(self) -> int:
        return int(self.meta["width"])

    @property
    def height(self) -> int:
        return int(self.meta["height"])

    @property
    def annotations(self) -> List[Dict[str, Any]]:
        """The annotations as drawn, each with its position in picture pixels."""
        return list(self.meta.get("annotations", []))

    def save(self, path: PathLike, *, overwrite: bool = False) -> str:
        """Write the picture to ``path`` (read back and compared before the rename); returns
        the path. The extension is not checked against :attr:`format`."""
        return _write_verified(path, self.data, overwrite)

    def _repr_png_(self) -> Optional[bytes]:
        return self.data if self.format == "png" else None

    def _repr_jpeg_(self) -> Optional[bytes]:
        return self.data if self.format == "jpeg" else None

    def _repr_svg_(self) -> Optional[str]:
        return self.data.decode("utf-8") if self.format == "svg" else None

    def __repr__(self) -> str:
        return (
            f"<openreadout.Preview {self.meta.get('kind', '?')} of "
            f"{os.path.basename(str(self.meta.get('path', '')))!r}, "
            f"{self.width}x{self.height} {self.format}>"
        )


class Figure:
    """A figure from :func:`plot`: render it as PNG, SVG or PDF as often as needed (the data
    is read once).

    In Jupyter it shows itself as a PNG drawn at twice its size, so it stays sharp on
    high-density screens. ``summary`` is the ``plot --json`` data: each panel's axes (label,
    unit, range, log), its series (label, colour, count, where the data came from) and the
    callouts it labelled, plus ``notes`` and ``warnings``.
    """

    def __init__(self, native: _native.NativeFigure) -> None:
        self._n = native
        self.summary: Dict[str, Any] = json.loads(native.summary_json)

    @property
    def size(self) -> Tuple[int, int]:
        """Width and height in pixels at scale 1."""
        w, h = self._n.size
        return int(w), int(h)

    def png(self, scale: float = 1.0) -> bytes:
        """PNG bytes; ``scale=2`` doubles every dimension."""
        return bytes(self._n.png(scale))

    def svg(self) -> str:
        """The figure as an SVG document."""
        return str(self._n.svg())

    def pdf(self) -> bytes:
        """The figure as a one-page vector PDF."""
        return bytes(self._n.pdf())

    def save(self, path: PathLike, *, scale: float = 2.0, overwrite: bool = False) -> str:
        """Write ``.png`` (at ``scale``), ``.svg`` or ``.pdf``, chosen by the extension, read
        back and compared before the rename; returns the path."""
        ext = os.path.splitext(os.fspath(path))[1].lower()
        if ext == ".png":
            data = self.png(scale)
        elif ext == ".svg":
            data = self.svg().encode("utf-8")
        elif ext == ".pdf":
            data = self.pdf()
        else:
            raise UsageError(
                f"cannot tell the figure format from {ext or 'no extension'!r}: "
                "use .png, .svg or .pdf"
            )
        return _write_verified(path, data, overwrite)

    def _repr_png_(self) -> Tuple[bytes, Dict[str, int]]:
        w, h = self.size
        return self.png(2.0), {"width": w, "height": h}

    def __repr__(self) -> str:
        w, h = self.size
        title = self.summary.get("title")
        panels = len(self.summary.get("panels", []))
        name = f" {title!r}" if title else ""
        return f"<openreadout.Figure{name}, {panels} panel{'s' if panels != 1 else ''}, {w}x{h}>"


def plot(spec: Union[Mapping[str, Any], str, PathLike, None] = None, /, **kwargs: Any) -> Figure:
    """Draw a figure from lab files (``openreadout plot``; the arguments of the MCP tool
    ``openreadout_plot``).

    ``spec`` is the spec as a dict, as JSON text, or the path of a ``.json`` file. Keyword
    arguments are added to it (or make it), so these are the same::

        openreadout.plot({"data": {"file": "run.mzML", "peaks": 1}, "title": "TIC"})
        openreadout.plot(data={"file": "run.mzML", "peaks": 1}, title="TIC")

    Each series names its data by ``file`` and a selector: ``trace``, ``sweep`` and ``channel``;
    a chromatogram (``"chromatogram": "tic"`` or ``mz`` for an XIC) or the spectrum at ``rt``;
    plate ``wells`` or layout ``groups``; FCS ``x`` and ``y`` parameters and ``populations``;
    ``show`` (``"fi"``, ``"dose_response"``, ``"standard_curve"``, ``"melt"``, ...) for an
    analysis result; a ``table`` file's columns; or inline ``values``. ``panels``, ``facet``
    (``"plate"`` lays wells out in plate positions), ``share``, ``x``/``y`` axis options,
    ``annotate`` (bands, lines, labelled points, arrows)
    and ``theme`` (``"light"`` or ``"dark"``) arrange it. The full reference is
    https://openreadout.github.io/openreadout/reference/commands/plot.html.

    Paths may be :class:`os.PathLike`. Save the result with :meth:`Figure.save`.
    """
    if spec is None:
        body: Dict[str, Any] = {}
    elif isinstance(spec, Mapping):
        body = dict(spec)
    else:
        text = os.fspath(spec)
        if text.lstrip().startswith("{"):
            body = json.loads(text)
        else:
            with open(text, encoding="utf-8") as f:
                body = json.load(f)
        if not isinstance(body, dict):
            raise UsageError("a plot spec is a JSON object")
    body.update({k: v for k, v in kwargs.items() if v is not None})
    return Figure(_native.plot(json.dumps(body, default=_jsonable)))
