use stellar_strkey::*;
fn main() {
    let source = Strkey::PublicKeyEd25519(stellar_strkey::ed25519::PublicKey([0u8; 32])).to_string();
    let contract = Strkey::Contract(stellar_strkey::Contract([1u8; 32])).to_string();
    println!("SOURCE: {}", source);
    println!("CONTRACT: {}", contract);
}