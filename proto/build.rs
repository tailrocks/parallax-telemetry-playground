fn main() -> Result<(), Box<dyn std::error::Error>> {
    let protoc_bin = protobuf_src::protoc();
    unsafe {
        std::env::set_var("PROTOC", &protoc_bin);
    }
    // tonic 0.14 moved prost codegen into the `tonic-prost-build` crate.
    tonic_prost_build::configure().compile_protos(&["pricing.proto", "payment.proto"], &["."])?;
    println!("cargo:rerun-if-changed=pricing.proto");
    println!("cargo:rerun-if-changed=payment.proto");
    Ok(())
}
