"""Typed views of the JSON the core returns.

These are :class:`typing.TypedDict` definitions of the documents described by the JSON Schemas
in ``docs/schema/``. At runtime they are ordinary ``dict`` objects; the types exist for editors
and type checkers. Keys marked optional are omitted when the file does not record a value.
"""

from __future__ import annotations

from typing import Any, Dict, List, Literal, Optional, Tuple, TypedDict

__all__ = [
    "ChannelInfo",
    "CheckReport",
    "Confidence",
    "DetectResult",
    "ExportReport",
    "FileInfo",
    "Finding",
    "FormatDescriptor",
    "ImageInfo",
    "InstrumentInfo",
    "LsEntry",
    "MosaicInfo",
    "ObjectiveInfo",
    "PhysicalSize",
    "PixelType",
    "Severity",
    "Source",
]

PixelType = Literal["int8", "int16", "int32", "uint8", "uint16", "uint32", "float", "double"]
"""OME-XML pixel type names."""

Confidence = Literal["high", "medium", "low"]
Source = Literal["spec", "vendor-impl", "prior-art", "inferred"]
Severity = Literal["info", "warning", "error"]


class FormatDescriptor(TypedDict):
    """Static description of a supported format."""

    id: str
    name: str
    vendor: str
    extensions: List[str]
    family: str
    can_read: bool
    can_write: bool
    confidence: Confidence
    known_gaps: List[str]


class _PhysicalSizeRequired(TypedDict):
    unit: str


class PhysicalSize(_PhysicalSizeRequired, total=False):
    """Pixel size in micrometres (``unit`` is always ``"µm"``)."""

    x: float
    y: float
    z: float


class _ChannelInfoRequired(TypedDict):
    index: int


class ChannelInfo(_ChannelInfoRequired, total=False):
    """One acquisition channel."""

    name: str
    fluorophore: str
    excitation_nm: float
    emission_nm: float
    emission_range_nm: Tuple[float, float]
    emission_band_start_nm: float
    emission_band_end_nm: float
    emission_band_center_nm: float
    color: str
    acquisition_mode: str
    exposure_ms: float


class ObjectiveInfo(TypedDict, total=False):
    """The objective lens."""

    model: str
    nominal_magnification: float
    lens_na: float
    immersion: str


class InstrumentInfo(TypedDict, total=False):
    """The instrument and software that produced the file."""

    manufacturer: str
    model: str
    software: str
    software_version: str
    detector: str


class _MosaicInfoRequired(TypedDict):
    tile_count: int
    stitched_on_read: bool


class MosaicInfo(_MosaicInfoRequired, total=False):
    """Mosaic (tiled) acquisition summary. Planes are returned stitched."""

    tile_width: int
    tile_height: int


class _ResolutionLevelRequired(TypedDict):
    level: int
    size_x: int
    size_y: int
    downsample_x: float
    downsample_y: float


class ResolutionLevel(_ResolutionLevelRequired, total=False):
    """One resolution level: size, downsampling relative to level 0, stored tile size (tiled
    images) and z count where it differs from the image's (Imaris)."""

    tile_width: int
    tile_height: int
    size_z: int


class _ImageInfoRequired(TypedDict):
    index: int
    size_x: int
    size_y: int
    size_z: int
    size_c: int
    size_t: int
    dimension_order: str
    pixel_type: PixelType
    samples_per_pixel: int
    physical_size: PhysicalSize
    channels: List[ChannelInfo]
    pyramid_levels: int
    plane_count: int


class ImageInfo(_ImageInfoRequired, total=False):
    """One image (scene, series or position) inside a file."""

    name: str
    time_increment_s: float
    objective: ObjectiveInfo
    instrument: InstrumentInfo
    acquired_at: str
    mosaic: MosaicInfo
    resolution_levels: List[ResolutionLevel]
    extra: Dict[str, Any]


class _FileInfoRequired(TypedDict):
    path: str
    size_bytes: int
    format: FormatDescriptor
    images: List[ImageInfo]
    plane_count: int


class Term(TypedDict):
    """An ontology term (PSI-MS, CHMO, FBbi or OBI): id and label."""

    id: str
    label: str


class _QuantityRequired(TypedDict):
    value: Any


class Quantity(_QuantityRequired, total=False):
    """A value with an optional unit and its UCUM code."""

    unit: str
    ucum: str


class Sample(TypedDict, total=False):
    """The sample's identity as the file records it."""

    id: str
    name: str
    well: str
    barcode: str
    sequence_position: str
    source_field: str


class ExperimentInstrument(TypedDict, total=False):
    """The instrument that made the measurement."""

    vendor: str
    model: str
    serial: str
    software: str
    software_version: str
    kind: Term


class Method(TypedDict, total=False):
    """Method name, technique and assay terms, and settings."""

    name: str
    technique: Term
    assay: Term
    parameters: Dict[str, Quantity]


class Acquisition(TypedDict, total=False):
    """When and by whom."""

    started_at: str
    ended_at: str
    operator: str
    duration_s: float
    comment: str


class _MeasurementRequired(TypedDict):
    kind: Literal["image", "table", "trace", "spectra"]
    indices: List[int]
    what: str


class Measurement(_MeasurementRequired, total=False):
    """One thing that was measured, in scientific words."""

    technique: Term
    terms: List[Term]
    parameters: Dict[str, Quantity]


class Experiment(TypedDict, total=False):
    """The experiment a file records (``info["experiment"]``; https://openreadout.github.io/openreadout/guides/metadata.html).

    ``provenance`` maps each value's path (``sample.id``) to ``{"source", "from"}``.
    """

    sample: Sample
    instrument: ExperimentInstrument
    method: Method
    acquisition: Acquisition
    measurements: List[Measurement]
    notes: List[str]
    provenance: Dict[str, Dict[str, str]]


class FileInfo(_FileInfoRequired, total=False):
    """Output of :func:`openreadout.info` (the CLI's ``info --json`` ``data``)."""

    format_version: str
    tables: List[Dict[str, Any]]
    spectra: List[Dict[str, Any]]
    traces: List[Dict[str, Any]]
    notes: List[str]
    experiment: Experiment


class _FindingRequired(TypedDict):
    severity: Severity
    code: str
    message: str


class Finding(_FindingRequired, total=False):
    """One integrity finding."""

    offset: int


class CheckReport(TypedDict):
    """Output of :meth:`openreadout.File.check`."""

    path: str
    format: str
    ok: bool
    checks_performed: List[str]
    findings: List[Finding]


class ExportReport(TypedDict):
    """Output of :meth:`openreadout.File.export`."""

    input: str
    output: str
    format: str
    images_written: int
    planes_written: int
    bytes_written: int
    verified: bool
    codec: Literal["none", "deflate", "lzw"]
    ome_xml_bytes: int


class _DetectResultRequired(TypedDict):
    path: str
    format: str
    name: str
    confidence: Literal["definite", "likely", "extension-only"]


class DetectResult(_DetectResultRequired, total=False):
    """Output of ``openreadout.info(path, view="format")``."""

    note: str


class _LsEntryRequired(TypedDict):
    kind: str
    name: str


class LsEntry(_LsEntryRequired, total=False):
    """One structural element of the container (the CLI's ``info --view structure``)."""

    offset: int
    size: int
    image: int
    details: Optional[Any]
