Blosc chunks from numcodecs' test fixtures (github.com/zarr-developers/numcodecs, commit
1f83681, `fixture/blosc/`, MIT licence): `codec.NN-config.json` is the numcodecs `Blosc`
configuration (05: lz4 + bit shuffle, 08: blosclz + bit shuffle, 09: snappy + bit shuffle,
11: lz4 + byte shuffle with 256-byte blocks), `codec.NN-encoded.MM.dat` the chunk numcodecs
wrote for `array.MM` (stored here as the array's raw little-endian C-order bytes, `.bin`,
converted from numcodecs' `.npy`).
