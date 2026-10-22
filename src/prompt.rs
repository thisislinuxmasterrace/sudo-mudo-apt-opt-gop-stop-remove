use std::io::{self, IsTerminal};

/// asks a yes/no question on stderr. anything other than "y" or "yes", including EOF, means no.
pub fn ask(question: &str) -> bool {
    read_line(&format!("{question} [y/N] "))
        .is_some_and(|answer| matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// prints `prompt` on stderr and reads one line, without its surrounding spaces. none means EOF or
/// a read error, and both are reported here.
pub fn read_line(prompt: &str) -> Option<String> {
    eprint!("{prompt}");

    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) => {
            eprintln!();
            if !io::stdin().is_terminal() {
                eprintln!("stdin is not a terminal: use -f to erase without a prompt");
            }
            None
        }
        Ok(_) => Some(answer.trim().to_owned()),
        Err(err) => {
            eprintln!();
            eprintln!("could not read the answer: {err}");
            None
        }
    }
}
