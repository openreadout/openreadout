# openreadout-jpegxr

Memory-safe JPEG XR (ITU-T T.832) decoder: a Rust port of the decoder in Microsoft's jxrlib, bit-exact with it, without `unsafe` code. OpenReadout uses it for Zeiss CZI files. This crate is part of [OpenReadout](https://github.com/openreadout/openreadout), which reads raw lab-instrument files without vendor software, prints JSON, exports to open formats and runs as an MCP server. Documentation: <https://openreadout.github.io/openreadout/formats/czi.html>. Licensed MIT OR Apache-2.0.
