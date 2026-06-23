fn main() -> Result<(), Box<dyn std::error::Error>> {
    // tonic 0.14 moved prost codegen into the `tonic-prost-build` crate.
    tonic_prost_build::configure().compile_protos(&["pricing.proto"], &["."])?;
    println!("cargo:rerun-if-changed=pricing.proto");
    Ok(())
}
