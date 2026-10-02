//! Tag numbers used by the reader (TIFF 6.0, TIFF Technical Notes, and the private tags the
//! microscopy conventions use). See `docs/formats/tiff.md` § Tags.

pub(crate) const NEW_SUBFILE_TYPE: u16 = 254;
pub(crate) const IMAGE_WIDTH: u16 = 256;
pub(crate) const IMAGE_LENGTH: u16 = 257;
pub(crate) const BITS_PER_SAMPLE: u16 = 258;
pub(crate) const COMPRESSION: u16 = 259;
pub(crate) const PHOTOMETRIC: u16 = 262;
pub(crate) const FILL_ORDER: u16 = 266;
pub(crate) const ORIENTATION: u16 = 274;
pub(crate) const IMAGE_DESCRIPTION: u16 = 270;
pub(crate) const MAKE: u16 = 271;
pub(crate) const MODEL: u16 = 272;
pub(crate) const STRIP_OFFSETS: u16 = 273;
pub(crate) const SAMPLES_PER_PIXEL: u16 = 277;
pub(crate) const ROWS_PER_STRIP: u16 = 278;
pub(crate) const STRIP_BYTE_COUNTS: u16 = 279;
pub(crate) const X_RESOLUTION: u16 = 282;
pub(crate) const Y_RESOLUTION: u16 = 283;
pub(crate) const PLANAR_CONFIGURATION: u16 = 284;
pub(crate) const RESOLUTION_UNIT: u16 = 296;
pub(crate) const SOFTWARE: u16 = 305;
pub(crate) const DATE_TIME: u16 = 306;
pub(crate) const PREDICTOR: u16 = 317;
pub(crate) const TILE_WIDTH: u16 = 322;
pub(crate) const TILE_LENGTH: u16 = 323;
pub(crate) const TILE_OFFSETS: u16 = 324;
pub(crate) const TILE_BYTE_COUNTS: u16 = 325;
pub(crate) const SUB_IFDS: u16 = 330;
pub(crate) const SAMPLE_FORMAT: u16 = 339;
pub(crate) const JPEG_TABLES: u16 = 347;
/// Zeiss LSM info record.
pub(crate) const LSM_INFO: u16 = 34412;
/// ImageJ binary metadata (byte counts / payload).
pub(crate) const IMAGEJ_META_COUNTS: u16 = 50838;
pub(crate) const IMAGEJ_META: u16 = 50839;
/// Micro-Manager per-plane JSON.
pub(crate) const MICROMANAGER_META: u16 = 51123;
/// Hamamatsu NDPI private tags.
pub(crate) const NDPI_FORMAT_FLAG: u16 = 65420;
pub(crate) const NDPI_MAGNIFICATION: u16 = 65421;
pub(crate) const NDPI_X_OFFSET_NM: u16 = 65422;
pub(crate) const NDPI_Y_OFFSET_NM: u16 = 65423;
pub(crate) const NDPI_Z_OFFSET_NM: u16 = 65424;
pub(crate) const NDPI_SLIDE_LABEL: u16 = 65427;
pub(crate) const NDPI_SCANNER_SERIAL: u16 = 65442;

/// Spec name of a tag, for listings and the vendor tree.
pub(crate) fn name(tag: u16) -> Option<&'static str> {
    Some(match tag {
        254 => "NewSubfileType",
        255 => "SubfileType",
        256 => "ImageWidth",
        257 => "ImageLength",
        258 => "BitsPerSample",
        259 => "Compression",
        262 => "PhotometricInterpretation",
        266 => "FillOrder",
        269 => "DocumentName",
        270 => "ImageDescription",
        271 => "Make",
        272 => "Model",
        273 => "StripOffsets",
        274 => "Orientation",
        277 => "SamplesPerPixel",
        278 => "RowsPerStrip",
        279 => "StripByteCounts",
        280 => "MinSampleValue",
        281 => "MaxSampleValue",
        282 => "XResolution",
        283 => "YResolution",
        284 => "PlanarConfiguration",
        285 => "PageName",
        286 => "XPosition",
        287 => "YPosition",
        296 => "ResolutionUnit",
        297 => "PageNumber",
        305 => "Software",
        306 => "DateTime",
        315 => "Artist",
        316 => "HostComputer",
        317 => "Predictor",
        318 => "WhitePoint",
        319 => "PrimaryChromaticities",
        320 => "ColorMap",
        322 => "TileWidth",
        323 => "TileLength",
        324 => "TileOffsets",
        325 => "TileByteCounts",
        330 => "SubIFDs",
        338 => "ExtraSamples",
        339 => "SampleFormat",
        340 => "SMinSampleValue",
        341 => "SMaxSampleValue",
        347 => "JPEGTables",
        529 => "YCbCrCoefficients",
        530 => "YCbCrSubSampling",
        531 => "YCbCrPositioning",
        532 => "ReferenceBlackWhite",
        700 => "XMP",
        32997 => "ImageDepth",
        32998 => "TileDepth",
        33432 => "Copyright",
        34665 => "ExifIFD",
        34675 => "ICCProfile",
        _ => return None,
    })
}
