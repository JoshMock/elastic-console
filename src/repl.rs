use crate::complete::EndpointCompleter;
use crate::highlight::SyntaxHighlighter;
use crate::http::execute;
use crate::output::print_response;
use crate::parse::parse;
use crate::Connection;
use reedline::{
    default_emacs_keybindings, ColumnarMenu, EditCommand, FileBackedHistory, KeyCode,
    KeyModifiers, MenuBuilder, Prompt, PromptEditMode, PromptHistorySearch,
    PromptHistorySearchStatus, Reedline, ReedlineEvent, ReedlineMenu, Signal,
};
use std::borrow::Cow;
use std::io::{stdout, Write};
use std::path::Path;

const HISTORY_SIZE: usize = 10000;

fn banner(conn: &Connection) -> String {
    format!("connected to {}", conn.url)
}

pub fn run_interactive(conn: &Connection, history_path: &Path) -> std::io::Result<()> {
    println!("{}", banner(conn));
    // Create parent directory if needed
    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let history = FileBackedHistory::with_file(HISTORY_SIZE, history_path.to_path_buf())?;

    let mut keybindings = default_emacs_keybindings();

    // Enter = InsertNewline (instead of Submit)
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Enter,
        ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
    );

    // Ctrl+Enter = Submit
    keybindings.add_binding(KeyModifiers::CONTROL, KeyCode::Enter, ReedlineEvent::Submit);

    // Alt+Enter = Submit (fallback for terminals that don't distinguish Ctrl+Enter)
    keybindings.add_binding(KeyModifiers::ALT, KeyCode::Enter, ReedlineEvent::Submit);

    // Tab = open completion menu or cycle through suggestions
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Tab,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::Menu("completion_menu".to_string()),
            ReedlineEvent::MenuNext,
        ]),
    );

    // Ctrl+C = clear input (handled by reedline as CtrlC signal)
    // Ctrl+D = exit (handled by reedline as CtrlD signal)

    let highlighter = SyntaxHighlighter::new();
    let completer = EndpointCompleter::new();
    let completion_menu = ColumnarMenu::default().with_name("completion_menu");

    let mut line_editor = Reedline::create()
        .with_history(Box::new(history))
        .with_edit_mode(Box::new(reedline::Emacs::new(keybindings)))
        .with_highlighter(Box::new(highlighter))
        .with_completer(Box::new(completer))
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(completion_menu)))
        .use_kitty_keyboard_enhancement(true);

    let prompt = EsPrompt;

    loop {
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                if buffer.trim().is_empty() {
                    continue;
                }

                match parse(&buffer) {
                    Ok(requests) => {
                        let mut out = stdout();
                        for req in &requests {
                            match execute(conn, req) {
                                Ok(resp) => print_response(&mut out, &resp),
                                Err(e) => {
                                    eprintln!("error: {}", e);
                                }
                            }
                        }
                        let _ = out.flush();
                    }
                    Err(e) => {
                        eprintln!("parse error: {}", e);
                        // Keep the buffer, user can fix and resubmit
                    }
                }
            }
            Ok(Signal::CtrlC) => {
                // Clear current input, continue
                continue;
            }
            Ok(Signal::CtrlD) => {
                // Exit
                break;
            }
            Ok(_) => {
                // Other signals (HostCommand, etc.) - ignore
                continue;
            }
            Err(e) => {
                eprintln!("error: {}", e);
                break;
            }
        }
    }

    Ok(())
}

struct EsPrompt;

impl Prompt for EsPrompt {
    fn render_prompt_left(&self) -> Cow<'_, str> {
        Cow::Borrowed("es> ")
    }

    fn render_prompt_right(&self) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_indicator(&self, _edit_mode: PromptEditMode) -> Cow<'_, str> {
        Cow::Borrowed("")
    }

    fn render_prompt_multiline_indicator(&self) -> Cow<'_, str> {
        Cow::Borrowed("... ")
    }

    fn render_prompt_history_search_indicator(
        &self,
        history_search: PromptHistorySearch,
    ) -> Cow<'_, str> {
        let prefix = match history_search.status {
            PromptHistorySearchStatus::Passing => "",
            PromptHistorySearchStatus::Failing => "failing ",
        };
        Cow::Owned(format!(
            "({}reverse-search: {}) ",
            prefix, history_search.term
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Auth;

    #[test]
    fn banner_shows_url_but_not_secret() {
        let conn = Connection {
            url: "https://es.example:9200".into(),
            auth: Auth::ApiKey("s3cret".into()),
        };
        let b = banner(&conn);
        assert!(b.contains("https://es.example:9200"));
        assert!(!b.contains("s3cret"));
    }
}
