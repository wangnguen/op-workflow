fn main() {
    let kind = std::env::args().nth(1).unwrap_or_else(|| "event".to_owned());
    opw_hook::run(&kind, false);
}
