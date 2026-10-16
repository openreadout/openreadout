//! Tags the reader interprets (PS3.6 keywords in the constant names, converted to Rust case).
#![allow(dead_code)] // the table names a few more tags than the reader reads today

pub(crate) const TRANSFER_SYNTAX_UID: u32 = 0x0002_0010;
pub(crate) const MEDIA_STORAGE_SOP_CLASS_UID: u32 = 0x0002_0002;
pub(crate) const IMPLEMENTATION_VERSION_NAME: u32 = 0x0002_0013;

pub(crate) const IMAGE_TYPE: u32 = 0x0008_0008;
pub(crate) const SOP_CLASS_UID: u32 = 0x0008_0016;
pub(crate) const SOP_INSTANCE_UID: u32 = 0x0008_0018;
pub(crate) const STUDY_DATE: u32 = 0x0008_0020;
pub(crate) const SERIES_DATE: u32 = 0x0008_0021;
pub(crate) const ACQUISITION_DATE: u32 = 0x0008_0022;
pub(crate) const CONTENT_DATE: u32 = 0x0008_0023;
pub(crate) const ACQUISITION_DATE_TIME: u32 = 0x0008_002A;
pub(crate) const STUDY_TIME: u32 = 0x0008_0030;
pub(crate) const SERIES_TIME: u32 = 0x0008_0031;
pub(crate) const ACQUISITION_TIME: u32 = 0x0008_0032;
pub(crate) const CONTENT_TIME: u32 = 0x0008_0033;
pub(crate) const MODALITY: u32 = 0x0008_0060;
pub(crate) const MANUFACTURER: u32 = 0x0008_0070;
pub(crate) const STUDY_DESCRIPTION: u32 = 0x0008_1030;
pub(crate) const SERIES_DESCRIPTION: u32 = 0x0008_103E;
pub(crate) const MANUFACTURER_MODEL_NAME: u32 = 0x0008_1090;

pub(crate) const PATIENT_SEX: u32 = 0x0010_0040;
pub(crate) const PATIENT_AGE: u32 = 0x0010_1010;
pub(crate) const PATIENT_SIZE: u32 = 0x0010_1020;
pub(crate) const PATIENT_WEIGHT: u32 = 0x0010_1030;
pub(crate) const PATIENT_SPECIES_DESCRIPTION: u32 = 0x0010_2201;

pub(crate) const BODY_PART_EXAMINED: u32 = 0x0018_0015;
pub(crate) const SCANNING_SEQUENCE: u32 = 0x0018_0020;
pub(crate) const SLICE_THICKNESS: u32 = 0x0018_0050;
pub(crate) const KVP: u32 = 0x0018_0060;
pub(crate) const REPETITION_TIME: u32 = 0x0018_0080;
pub(crate) const ECHO_TIME: u32 = 0x0018_0081;
pub(crate) const INVERSION_TIME: u32 = 0x0018_0082;
pub(crate) const MAGNETIC_FIELD_STRENGTH: u32 = 0x0018_0087;
pub(crate) const SPACING_BETWEEN_SLICES: u32 = 0x0018_0088;
pub(crate) const SOFTWARE_VERSIONS: u32 = 0x0018_1020;
pub(crate) const PROTOCOL_NAME: u32 = 0x0018_1030;
pub(crate) const FRAME_TIME: u32 = 0x0018_1063;
pub(crate) const RADIOPHARMACEUTICAL_INFORMATION_SEQUENCE: u32 = 0x0054_0016;
pub(crate) const RADIOPHARMACEUTICAL: u32 = 0x0018_0031;
pub(crate) const IMAGER_PIXEL_SPACING: u32 = 0x0018_1164;
pub(crate) const FLIP_ANGLE: u32 = 0x0018_1314;
pub(crate) const CONVOLUTION_KERNEL: u32 = 0x0018_1210;
pub(crate) const EXPOSURE_TIME: u32 = 0x0018_1150;
pub(crate) const X_RAY_TUBE_CURRENT: u32 = 0x0018_1151;
pub(crate) const SEQUENCE_OF_ULTRASOUND_REGIONS: u32 = 0x0018_6011;
pub(crate) const PHYSICAL_UNITS_X_DIRECTION: u32 = 0x0018_6024;
pub(crate) const PHYSICAL_UNITS_Y_DIRECTION: u32 = 0x0018_6026;
pub(crate) const PHYSICAL_DELTA_X: u32 = 0x0018_602C;
pub(crate) const PHYSICAL_DELTA_Y: u32 = 0x0018_602E;
pub(crate) const TRIGGER_TIME: u32 = 0x0018_1060;

pub(crate) const STUDY_INSTANCE_UID: u32 = 0x0020_000D;
pub(crate) const SERIES_INSTANCE_UID: u32 = 0x0020_000E;
pub(crate) const SERIES_NUMBER: u32 = 0x0020_0011;
pub(crate) const ACQUISITION_NUMBER: u32 = 0x0020_0012;
pub(crate) const INSTANCE_NUMBER: u32 = 0x0020_0013;
pub(crate) const IMAGE_POSITION_PATIENT: u32 = 0x0020_0032;
pub(crate) const IMAGE_ORIENTATION_PATIENT: u32 = 0x0020_0037;
pub(crate) const FRAME_OF_REFERENCE_UID: u32 = 0x0020_0052;
pub(crate) const TEMPORAL_POSITION_IDENTIFIER: u32 = 0x0020_0100;
pub(crate) const NUMBER_OF_TEMPORAL_POSITIONS: u32 = 0x0020_0105;
pub(crate) const SLICE_LOCATION: u32 = 0x0020_1041;
pub(crate) const STACK_ID: u32 = 0x0020_9056;
pub(crate) const IN_STACK_POSITION_NUMBER: u32 = 0x0020_9057;
pub(crate) const FRAME_CONTENT_SEQUENCE: u32 = 0x0020_9111;
pub(crate) const PLANE_POSITION_SEQUENCE: u32 = 0x0020_9113;
pub(crate) const PLANE_ORIENTATION_SEQUENCE: u32 = 0x0020_9116;
pub(crate) const TEMPORAL_POSITION_INDEX: u32 = 0x0020_9128;
pub(crate) const DIMENSION_INDEX_VALUES: u32 = 0x0020_9157;

pub(crate) const SAMPLES_PER_PIXEL: u32 = 0x0028_0002;
pub(crate) const PHOTOMETRIC_INTERPRETATION: u32 = 0x0028_0004;
pub(crate) const PLANAR_CONFIGURATION: u32 = 0x0028_0006;
pub(crate) const NUMBER_OF_FRAMES: u32 = 0x0028_0008;
pub(crate) const FRAME_INCREMENT_POINTER: u32 = 0x0028_0009;
pub(crate) const ROWS: u32 = 0x0028_0010;
pub(crate) const COLUMNS: u32 = 0x0028_0011;
pub(crate) const PIXEL_SPACING: u32 = 0x0028_0030;
pub(crate) const BITS_ALLOCATED: u32 = 0x0028_0100;
pub(crate) const BITS_STORED: u32 = 0x0028_0101;
pub(crate) const HIGH_BIT: u32 = 0x0028_0102;
pub(crate) const PIXEL_REPRESENTATION: u32 = 0x0028_0103;
pub(crate) const BURNED_IN_ANNOTATION: u32 = 0x0028_0301;
pub(crate) const PIXEL_SPACING_CALIBRATION_TYPE: u32 = 0x0028_0A02;
pub(crate) const WINDOW_CENTER: u32 = 0x0028_1050;
pub(crate) const WINDOW_WIDTH: u32 = 0x0028_1051;
pub(crate) const RESCALE_INTERCEPT: u32 = 0x0028_1052;
pub(crate) const RESCALE_SLOPE: u32 = 0x0028_1053;
pub(crate) const RESCALE_TYPE: u32 = 0x0028_1054;
pub(crate) const WINDOW_CENTER_WIDTH_EXPLANATION: u32 = 0x0028_1055;
pub(crate) const RED_PALETTE_DESCRIPTOR: u32 = 0x0028_1101;
pub(crate) const GREEN_PALETTE_DESCRIPTOR: u32 = 0x0028_1102;
pub(crate) const BLUE_PALETTE_DESCRIPTOR: u32 = 0x0028_1103;
pub(crate) const RED_PALETTE_DATA: u32 = 0x0028_1201;
pub(crate) const GREEN_PALETTE_DATA: u32 = 0x0028_1202;
pub(crate) const BLUE_PALETTE_DATA: u32 = 0x0028_1203;
pub(crate) const SEGMENTED_RED_PALETTE_DATA: u32 = 0x0028_1221;
pub(crate) const LOSSY_IMAGE_COMPRESSION: u32 = 0x0028_2110;
pub(crate) const MODALITY_LUT_SEQUENCE: u32 = 0x0028_3000;
pub(crate) const PIXEL_MEASURES_SEQUENCE: u32 = 0x0028_9110;
pub(crate) const FRAME_VOI_LUT_SEQUENCE: u32 = 0x0028_9132;
pub(crate) const PIXEL_VALUE_TRANSFORMATION_SEQUENCE: u32 = 0x0028_9145;
pub(crate) const EXTENDED_OFFSET_TABLE: u32 = 0x7FE0_0001;
pub(crate) const EXTENDED_OFFSET_TABLE_LENGTHS: u32 = 0x7FE0_0002;

pub(crate) const SHARED_FUNCTIONAL_GROUPS_SEQUENCE: u32 = 0x5200_9229;
pub(crate) const PER_FRAME_FUNCTIONAL_GROUPS_SEQUENCE: u32 = 0x5200_9230;

pub(crate) const UNITS: u32 = 0x0054_1001;
pub(crate) const DECAY_CORRECTION: u32 = 0x0054_1102;
pub(crate) const SLICE_VECTOR: u32 = 0x0054_0080;
pub(crate) const NUMBER_OF_SLICES: u32 = 0x0054_0081;
pub(crate) const GRID_FRAME_OFFSET_VECTOR: u32 = 0x3004_000C;
pub(crate) const DOSE_GRID_SCALING: u32 = 0x3004_000E;
pub(crate) const DOSE_UNITS: u32 = 0x3004_0002;

// Whole-slide microscopy (PS3.3 C.8.12.4)
pub(crate) const IMAGED_VOLUME_WIDTH: u32 = 0x0048_0001;
pub(crate) const IMAGED_VOLUME_HEIGHT: u32 = 0x0048_0002;
pub(crate) const TOTAL_PIXEL_MATRIX_COLUMNS: u32 = 0x0048_0006;
pub(crate) const TOTAL_PIXEL_MATRIX_ROWS: u32 = 0x0048_0007;
pub(crate) const SPECIMEN_DESCRIPTION_SEQUENCE: u32 = 0x0040_0560;
pub(crate) const SPECIMEN_IDENTIFIER: u32 = 0x0040_0551;
pub(crate) const SPECIMEN_SHORT_DESCRIPTION: u32 = 0x0040_0600;
pub(crate) const CONTAINER_IDENTIFIER: u32 = 0x0040_0512;
pub(crate) const OPTICAL_PATH_SEQUENCE: u32 = 0x0048_0105;
pub(crate) const OPTICAL_PATH_IDENTIFIER: u32 = 0x0048_0106;
pub(crate) const OPTICAL_PATH_DESCRIPTION: u32 = 0x0048_0107;
pub(crate) const OBJECTIVE_LENS_POWER: u32 = 0x0048_0112;
pub(crate) const OBJECTIVE_LENS_NUMERICAL_APERTURE: u32 = 0x0048_0113;
pub(crate) const ILLUMINATION_WAVE_LENGTH: u32 = 0x0022_0055;
pub(crate) const PLANE_POSITION_SLIDE_SEQUENCE: u32 = 0x0048_021A;
pub(crate) const COLUMN_POSITION_IN_TOTAL_IMAGE_PIXEL_MATRIX: u32 = 0x0048_021E;
pub(crate) const ROW_POSITION_IN_TOTAL_IMAGE_PIXEL_MATRIX: u32 = 0x0048_021F;
pub(crate) const DIMENSION_ORGANIZATION_TYPE: u32 = 0x0020_9311;
pub(crate) const TOTAL_PIXEL_MATRIX_FOCAL_PLANES: u32 = 0x0048_0303;
pub(crate) const NUMBER_OF_OPTICAL_PATHS: u32 = 0x0048_0302;

// DICOMDIR (PS3.3 F.5)
pub(crate) const DIRECTORY_RECORD_SEQUENCE: u32 = 0x0004_1220;
pub(crate) const DIRECTORY_RECORD_TYPE: u32 = 0x0004_1430;
pub(crate) const REFERENCED_FILE_ID: u32 = 0x0004_1500;
