//! Mint a session token with the server's own signing code.
//!
//! The parity replay needs a bearer token the Rust build accepts. Minting it
//! with `star_auth::create_token` — the same function the app uses — means the
//! replay and the server can never disagree about the token format.
//!
//! ```sh
//! STAR_JWT_SECRET=... cargo run --bin mint_token -- <user_id> <role>
//! ```
//!
//! Prints the token on stdout, nothing else.

fn main() {
    let mut args = std::env::args().skip(1);
    let user_id: i64 = args.next().and_then(|a| a.parse().ok()).unwrap_or_else(|| {
        eprintln!("usage: mint_token <user_id> [role]");
        std::process::exit(2);
    });
    let role = args.next().unwrap_or_else(|| "teacher".to_string());

    let secret = std::env::var("STAR_JWT_SECRET").unwrap_or_else(|_| {
        eprintln!("mint_token: STAR_JWT_SECRET must be set");
        std::process::exit(2);
    });

    match star_auth::create_token(user_id, &role, &secret) {
        Ok(token) => println!("{token}"),
        Err(err) => {
            eprintln!("mint_token: {err}");
            std::process::exit(1);
        }
    }
}
