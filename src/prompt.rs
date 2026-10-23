use std::io::{self, IsTerminal};

/// asks a yes/no question on stderr. anything other than "y" or "yes", including EOF, means no.
pub fn ask(question: &str) -> bool {
    read_line(&format!("{question} [y/N] "))
        .is_some_and(|answer| matches!(answer.to_ascii_lowercase().as_str(), "y" | "yes"))
}

/// asks for `expected` to be typed exactly, on stderr. anything else, including EOF, means no.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub fn ask_exact(question: &str, expected: &str) -> bool {
    read_answer(
        &format!("{question} "),
        "stdin is not a terminal: a device is erased only after its path is typed, run it from a terminal",
    )
    .is_some_and(|answer| answer == expected)
}

/// prints `prompt` on stderr and reads one line, without its surrounding spaces. none means EOF or
/// a read error, and both are reported here.
pub fn read_line(prompt: &str) -> Option<String> {
    read_answer(
        prompt,
        "stdin is not a terminal: use -f to erase without a prompt",
    )
}

fn read_answer(prompt: &str, eof_hint: &str) -> Option<String> {
    eprint!("{prompt}");

    let mut answer = String::new();
    match io::stdin().read_line(&mut answer) {
        Ok(0) => {
            eprintln!();
            if !io::stdin().is_terminal() {
                eprintln!("{eof_hint}");
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
