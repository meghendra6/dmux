mod cli;
mod paths;
mod protocol;

fn main() {
    match cli::parse_args(std::env::args()) {
        Ok(command) => {
            eprintln!("dmux command is not implemented yet: {command:?}");
            std::process::exit(2);
        }
        Err(err) => {
            eprintln!("{err}");
            std::process::exit(2);
        }
    }
}
