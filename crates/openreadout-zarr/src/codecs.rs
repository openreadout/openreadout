//! Pure-Rust decode-only chunk codecs registered with `zarrs` at run time.
//!
//! `zarrs` implements `blosc` and `zstd` on top of C libraries (`blosc-src`, `zstd-sys`), which
//! the static musl builds avoid. The workspace builds `zarrs` without those features, and this
//! module registers decode-only replacements backed by `openreadout-codecs` (Blosc 1 with
//! blosclz/lz4/zlib/zstd inside, zstd through `ruzstd`) plus the numcodecs `lz4` compressor.
//! Writing with them is refused: the OME-Zarr writer only uses `gzip`.

use std::borrow::Cow;
use std::sync::{Arc, Once};

use zarrs::array::codec::api::{
    ArrayBytesRaw, BytesRepresentation, BytesToBytesCodecTraits, Codec, CodecError,
    CodecMetadataOptions, CodecOptions, CodecRuntimePluginV2, CodecRuntimePluginV3, CodecTraits,
    PartialDecoderCapability, PartialEncoderCapability, RecommendedConcurrency, register_codec_v2,
    register_codec_v3,
};
use zarrs::metadata::Configuration;
use zarrs::plugin::{ExtensionName, ZarrVersion};

/// Which decoder a [`DecodeOnly`] codec runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChunkCompressor {
    /// Blosc 1 chunks (`blosc`, Zarr v2 and v3).
    Blosc,
    /// A zstd frame (`zstd`, Zarr v2 and v3).
    Zstd,
    /// numcodecs `lz4`: a 4-byte decoded size, then an LZ4 block (Zarr v2).
    Lz4,
}

impl ChunkCompressor {
    /// The codec name in array metadata.
    pub fn name(self) -> &'static str {
        match self {
            ChunkCompressor::Blosc => "blosc",
            ChunkCompressor::Zstd => "zstd",
            ChunkCompressor::Lz4 => "lz4",
        }
    }
}

/// A bytes-to-bytes codec that only decodes.
#[derive(Debug, Clone)]
pub struct DecodeOnly {
    pub compressor: ChunkCompressor,
    configuration: Configuration,
}

impl ExtensionName for DecodeOnly {
    fn name(&self, _version: ZarrVersion) -> Option<Cow<'static, str>> {
        Some(Cow::Borrowed(self.compressor.name()))
    }
}

impl CodecTraits for DecodeOnly {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn configuration(
        &self,
        _version: ZarrVersion,
        _options: &CodecMetadataOptions,
    ) -> Option<Configuration> {
        Some(self.configuration.clone())
    }
    fn partial_decoder_capability(&self) -> PartialDecoderCapability {
        PartialDecoderCapability {
            partial_read: false,
            partial_decode: false,
        }
    }
    fn partial_encoder_capability(&self) -> PartialEncoderCapability {
        PartialEncoderCapability {
            partial_encode: false,
        }
    }
}

impl BytesToBytesCodecTraits for DecodeOnly {
    fn into_dyn(self: Arc<Self>) -> Arc<dyn BytesToBytesCodecTraits> {
        self as Arc<dyn BytesToBytesCodecTraits>
    }
    fn recommended_concurrency(
        &self,
        _decoded_representation: &BytesRepresentation,
    ) -> Result<RecommendedConcurrency, CodecError> {
        Ok(RecommendedConcurrency::new_maximum(1))
    }
    fn encoded_representation(
        &self,
        _decoded_representation: &BytesRepresentation,
    ) -> BytesRepresentation {
        BytesRepresentation::UnboundedSize
    }
    fn encode<'a>(
        &self,
        _decoded_value: ArrayBytesRaw<'a>,
        _options: &CodecOptions,
    ) -> Result<ArrayBytesRaw<'a>, CodecError> {
        Err(CodecError::Other(format!(
            "the {} codec is decode-only in openreadout",
            self.compressor.name()
        )))
    }
    fn decode<'a>(
        &self,
        encoded_value: ArrayBytesRaw<'a>,
        _decoded_representation: &BytesRepresentation,
        _options: &CodecOptions,
    ) -> Result<ArrayBytesRaw<'a>, CodecError> {
        let out = match self.compressor {
            ChunkCompressor::Blosc => openreadout_codecs::blosc_decode(&encoded_value),
            ChunkCompressor::Zstd => openreadout_codecs::zstd_decode(&encoded_value, 0),
            ChunkCompressor::Lz4 => openreadout_codecs::lz4_sized_decode(&encoded_value),
        }
        .map_err(|e| CodecError::Other(e.to_string()))?;
        Ok(Cow::Owned(out))
    }
}

fn make(c: ChunkCompressor, configuration: &Configuration) -> Codec {
    Codec::BytesToBytes(Arc::new(DecodeOnly {
        compressor: c,
        configuration: configuration.clone(),
    }))
}

/// Register the decoders once per process (later registrations take precedence over built-in
/// `zarrs` codecs of the same name, none of which are compiled in).
pub fn register() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        for c in [ChunkCompressor::Blosc, ChunkCompressor::Zstd] {
            register_codec_v3(CodecRuntimePluginV3::new(
                move |name| name == c.name(),
                move |m: &zarrs::metadata::v3::MetadataV3| {
                    Ok(make(c, &m.configuration().cloned().unwrap_or_default()))
                },
            ));
        }
        for c in [
            ChunkCompressor::Blosc,
            ChunkCompressor::Zstd,
            ChunkCompressor::Lz4,
        ] {
            register_codec_v2(CodecRuntimePluginV2::new(
                move |name| name == c.name(),
                move |m: &zarrs::metadata::v2::MetadataV2| Ok(make(c, m.configuration())),
            ));
        }
    });
}
