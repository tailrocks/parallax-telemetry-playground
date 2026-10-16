fn main() {
    println!("cargo:rerun-if-changed=pricing.proto");
    println!("cargo:rerun-if-changed=payment.proto");
}
