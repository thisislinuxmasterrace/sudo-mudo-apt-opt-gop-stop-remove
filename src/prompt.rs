use std::io;

/// asks a yes/no question on stderr. anything other than "y" or "yes", including EOF, means no.
pub fn ask(question: &str) -> bool {
    eprint!("{question} [y/N] ");

    let mut answer = String::new();
    if io::stdin().read_line(&mut answer).is_err() {
        return false;
    }
    matches!(answer.trim().to_ascii_lowercase().as_str(), "y" | "yes")
}
