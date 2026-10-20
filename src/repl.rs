use crate::http::execute;
use crate::output::print_response;
use crate::parse::parse;
use crate::Connection;
use reedline::{
    default_emacs_keybindings, EditCommand, FileBackedHistory, KeyCode, KeyModifiers, Prompt,
    PromptEditMode, PromptHistorySearch, PromptHistorySearchStatus, Reedline, ReedlineEvent,
    Signal,
};
use std::borrow::Cow;
use std::io::{stdout, Write};
use std::path::Path;

const HISTORY_SIZE: usize = 10000;

pub fn run_interactive(conn: &Connection, history_path: &Path) -> std::io::Result<()> {
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

    // Ctrl+C = clear input (handled by reedline as CtrlC signal)
    // Ctrl+D = exit (handled by reedline as CtrlD signal)

    let mut line_editor = Reedline::create()
        .with_history(Box::new(history))
        .with_edit_mode(Box::new(reedline::Emacs::new(keybindings)))
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
