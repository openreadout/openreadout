"""openreadout: read raw lab-instrument files (Zeiss CZI, Nikon ND2, Leica LIF, FCS, Bruker NMR,
JCAMP-DX) in Python.

Quick start::

    import openreadout

    meta = openreadout.info("run42.czi")            # header-only summary (dict)
    with openreadout.File("run42.czi") as f:
        plane = f.read_plane(image=0, c=1, z=4)       # numpy (Y, X) or (Y, X, S)
        stack = f.read_image(0)                       # numpy (T, C, Z, Y, X[, S])
        lazy = f.to_dask(0)                           # dask, one chunk per plane
        xarr = f.to_xarray(0)                         # labelled, lazy
        f.export("run42.ome.tiff")                    # verified OME-TIFF
    with openreadout.File("tube1.fcs") as f:
        events = f.read_table(0)                      # numpy (events, parameters), float64
    with openreadout.File("cell3.abf") as f:
        sweep = f.read_trace(sweep=2)                 # numpy (channels, samples), physical units
    with openreadout.File("nmr/sucrose/1") as f:    # Bruker experiment directory
        fid = f.read_trace(0)                         # numpy (channels=real,imag, samples)
        spec = f.read_trace(1, channel="real")        # processed 1r, numpy (samples,)
        ppm = f.trace_axis(1)                         # chemical shift of each point
    peaks = openreadout.analyze("nmr/sucrose/1", "nmr-peaks")    # ``openreadout analyze``

Functions and methods are named after the CLI commands (``info`` with ``view=``, ``check``,
``stats``, ``spectra``, ``analyze``, ``export``, ``batch``, ``link``).

Metadata is the same JSON the ``openreadout`` CLI prints
(https://openreadout.github.io/openreadout/getting-started/reading-json.html), typed as
:class:`~openreadout.FileInfo`. Errors are :class:`~openreadout.OpenReadoutError`
subclasses that are also the matching built-in exception (``OSError``, ``ValueError``, ...).
"""

from __future__ import annotations

import json
import os
from typing import Any, Dict, List, Optional, cast

from . import _native
from ._batch import BatchResult, batch, link
from ._errors import (
    CorruptFileError,
    InstrumentFileNotFoundError,
    InstrumentIOError,
    OpenReadoutError,
    UnknownFormatError,
    UnsupportedFeatureError,
    UsageError,
)
from ._file import (
    _PATH_KINDS,
    DEFAULT_BUFFER_NAME,
    DIMS,
    File,
    PathLike,
    SourceLike,
    _fileobj_name,
    analyze_path,
)
from ._types import (
    ChannelInfo,
    CheckReport,
    DetectResult,
    Experiment,
    ExportReport,
    FileInfo,
    Finding,
    FormatDescriptor,
    ImageInfo,
    LsEntry,
    PhysicalSize,
)

__version__: str = _native.__version__

__all__ = [
    "DIMS",
    "BatchResult",
    "ChannelInfo",
    "CheckReport",
    "CorruptFileError",
    "DetectResult",
    "Experiment",
    "ExportReport",
    "File",
    "FileInfo",
    "Finding",
    "FormatDescriptor",
    "ImageInfo",
    "InstrumentFileNotFoundError",
    "InstrumentIOError",
    "LsEntry",
    "OpenReadoutError",
    "PathLike",
    "PhysicalSize",
    "SourceLike",
    "UnknownFormatError",
    "UnsupportedFeatureError",
    "UsageError",
    "__version__",
    "analyze",
    "batch",
    "export",
    "formats",
    "info",
    "link",
    "open",
]


def open(path: SourceLike, *, name: Optional[str] = None) -> File:
    """Open an instrument file; the format is detected from its signature.

    ``path`` may also be the file's bytes or a binary file-like object (``read`` and ``seek``),
    e.g. ``openreadout.open(blob, name="run.czi")`` or ``openreadout.open(fsspec_file)``.
    Equivalent to ``File(path, name=name)``; use it as a context manager to close the handle
    promptly.
    """
    return File(path, name=name)


#: The views of :func:`info`, as ``openreadout info --view`` names them.
INFO_VIEWS = ("summary", "full", "structure", "explain", "format")


def info(
    path: SourceLike,
    *,
    view: str = "summary",
    ask: Optional[str] = None,
    vendor: bool = True,
    name: Optional[str] = None,
) -> Any:
    """What a file holds (``openreadout info --view VIEW --json``'s ``data``). Opens, reads the
    headers, and closes the file; no pixel data is decoded. Accepts what :func:`open` accepts.

    ``view``:

    - ``"summary"`` (default): the normalized summary, a :class:`FileInfo` (as ``File.info``);
    - ``"full"``: ``{"file": <summary with per-frame records>, "vendor": <the vendor's own
      metadata tree> (left out with ``vendor=False``), "provenance": <where each normalized
      field came from>}``;
    - ``"structure"``: ``{"path", "format", "entries"}``, the container's elements in file
      order;
    - ``"explain"``: what the file is, in sentences; ``ask`` answers a question in words,
      naming the fields it used;
    - ``"format"``: a :class:`DetectResult`, from the first bytes only (the file is not
      parsed).

    Raises:
        UnknownFormatError: No supported format matches (also a ``ValueError``).
    """
    if view not in INFO_VIEWS:
        raise UsageError(f"view must be one of {', '.join(INFO_VIEWS)}; not {view!r}")
    if ask is not None and view != "explain":
        raise UsageError("ask= goes with view='explain'")
    if view == "format":
        return _detect(path, name)
    with File(path, name=name) as f:
        if view == "summary":
            return f.info
        if view == "full":
            return json.loads(f._n.full_json(vendor))
        if view == "structure":
            return {"path": f.path, "format": f.format, "entries": f.entries()}
        return json.loads(f._n.explain_json(ask))


def _detect(path: SourceLike, name: Optional[str]) -> DetectResult:
    if isinstance(path, (str, os.PathLike)):
        return cast(DetectResult, json.loads(_native.detect_json(os.fspath(path))))
    if isinstance(path, (bytes, bytearray, memoryview)):
        data = path if isinstance(path, bytes) else bytes(path)
        out = _native.detect_bytes_json(data, name or DEFAULT_BUFFER_NAME)
    else:
        out = _native.detect_fileobj_json(path, name or _fileobj_name(path), None)
    return cast(DetectResult, json.loads(out))


def analyze(
    path: SourceLike, kind: str, *, name: Optional[str] = None, **options: Any
) -> Dict[str, Any]:
    """An analysis with a documented method (``openreadout analyze KIND FILE --json``'s
    ``data``, as a dict).

    ``kind`` is one of ``"peaks"`` (chromatographic peaks, compound lists, bands and regions of
    IR/Raman/UV-Vis/NMR spectra), ``"chromatogram"`` (TIC, BPC, XIC, SRM, detector traces;
    ``rt_min`` and ``intensity`` as NumPy arrays), ``"nmr-peaks"``, ``"ephys-features"``
    (patch clamp), ``"spikes"`` (extracellular), ``"qpcr"`` (Cq, ΔΔCq, standard curves),
    ``"assay"`` (plate-reader assays) or ``"gate"`` (flow-cytometry gating). The keyword
    options are the ``options`` of the MCP tool ``openreadout_analyze`` for that kind
    (https://openreadout.github.io/openreadout/reference/mcp.html); ``from_=`` stands for the
    option ``from`` (a Python keyword). Indices are zero-based. An option the kind does not
    take is a :class:`UsageError` naming the ones it does.

    ``qpcr``, ``assay`` and ``gate`` take a path; the others also what :func:`open` accepts.
    With ``kind="assay"`` and ``plot=True`` the fitted curve comes back as PNG bytes under
    ``"plot_png"`` (``None`` when nothing was fitted).

    Examples::

        openreadout.analyze("run.raw", "chromatogram", mz=[195.0877], ppm=5)
        openreadout.analyze("run.D", "peaks", traces=[0], min_snr=10)
        openreadout.analyze("nmr/1", "nmr-peaks", from_="fid", integrate=[(3.2, 3.9)])
        openreadout.analyze("cell.abf", "ephys-features")
        openreadout.analyze("plate.eds", "qpcr", ddcq=True)
        openreadout.analyze("elisa.xlsx", "assay", analysis="curve", layout="map.csv", model="4pl")
        openreadout.analyze("tube.fcs", "gate", workspace="panel.wsp", medians=["Comp-FITC-A"])
    """
    if kind in _PATH_KINDS or isinstance(path, (str, os.PathLike)):
        if not isinstance(path, (str, os.PathLike)):
            raise UsageError(f"analysis {kind!r} needs a path")
        return analyze_path(path, kind, options)
    with File(path, name=name) as f:
        return f.analyze(kind, **options)


def export(
    path: PathLike, output: PathLike, *, to: Optional[str] = None, **options: Any
) -> ExportReport:
    """Export a file to an open format (``openreadout export``): :meth:`File.export` on
    ``path``, which documents ``to`` and the options."""
    with File(path) as f:
        return f.export(output, to=to, **options)


def formats() -> List[FormatDescriptor]:
    """Formats this build reads, with confidence and known gaps."""
    return cast(List[FormatDescriptor], json.loads(_native.formats_json())["formats"])
