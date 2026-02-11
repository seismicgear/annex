use ark_bn254::Fr;
use ark_ff::{BigInteger, PrimeField};

fn main() {
    let val = Fr::from(5u64);
    let bytes = val.into_bigint().to_bytes_be();
    println!("Bytes len: {}", bytes.len());
    println!("Bytes: {:?}", bytes);
}
