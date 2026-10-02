// Forward R's routine registration to the Rust static library (crates/openreadout-r), whose
// extendr_module! defines R_init_openreadout_extendr. Referencing it here also keeps the linker
// from dropping the library.

void R_init_openreadout_extendr(void *dll);

void R_init_openreadout(void *dll) {
    R_init_openreadout_extendr(dll);
}
