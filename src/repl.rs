use crate::complete::EndpointCompleter;
use crate::highlight::SyntaxHighlighter;
use crate::http::execute;
use crate::output::print_response;
use crate::parse::select_request_at_cursor;
use crate::vars::{format_vars, is_command, is_vars_command, parse_set_command, substitute_request};
use crate::Connection;
use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use reedline::{
    default_emacs_keybindings, ColumnarMenu, Keybindings, EditCommand, FileBackedHistory, KeyCode,
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

fn keybindings() -> Keybindings {
    let mut keybindings = default_emacs_keybindings();

    // Enter = accept the open completion menu's selection, else InsertNewline (instead of Submit)
    keybindings.add_binding(
        KeyModifiers::NONE,
        KeyCode::Enter,
        ReedlineEvent::UntilFound(vec![
            ReedlineEvent::MenuAccept,
            ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
        ]),
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

    // Shift+Tab = cycle the completion menu backwards
    keybindings.add_binding(KeyModifiers::SHIFT, KeyCode::BackTab, ReedlineEvent::MenuPrevious);

    keybindings
}

/// Picks the request under the cursor and substitutes `${var}`s in it.
fn request_to_run(
    buffer: &str,
    cursor: usize,
    vars: &HashMap<String, String>,
) -> Result<Option<crate::parse::Request>, crate::parse::ParseError> {
    Ok(select_request_at_cursor(buffer, cursor)?.map(|r| substitute_request(r, vars)))
}

pub fn run_interactive(conn: &Connection, history_path: &Path) -> std::io::Result<()> {
    println!("{}", banner(conn));
    // Create parent directory if needed
    if let Some(parent) = history_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let history = FileBackedHistory::with_file(HISTORY_SIZE, history_path.to_path_buf())?;

    let keybindings = keybindings();

    // Ctrl+C = clear input (handled by reedline as CtrlC signal)
    // Ctrl+D = exit (handled by reedline as CtrlD signal)

    // ponytail: shared cursor position for run-under-cursor (approach b from PLAN)
    let cursor_pos = Arc::new(AtomicUsize::new(0));
    let highlighter = SyntaxHighlighter::with_cursor_tracker(cursor_pos.clone());
    let completer = EndpointCompleter::with_connection(conn.clone());
    let completion_menu = ColumnarMenu::default().with_name("completion_menu");

    let mut line_editor = Reedline::create()
        .with_history(Box::new(history))
        .with_edit_mode(Box::new(reedline::Emacs::new(keybindings)))
        .with_highlighter(Box::new(highlighter))
        .with_completer(Box::new(completer))
        .with_menu(ReedlineMenu::EngineCompleter(Box::new(completion_menu)))
        .use_kitty_keyboard_enhancement(true);

    let prompt = EsPrompt;
    let mut vars: HashMap<String, String> = HashMap::new();

    loop {
        match line_editor.read_line(&prompt) {
            Ok(Signal::Success(buffer)) => {
                if buffer.trim().is_empty() {
                    continue;
                }

                // If buffer starts with :, treat each line as a command
                if is_command(&buffer) {
                    for line in buffer.lines() {
                        if let Some((name, val)) = parse_set_command(line) {
                            vars.insert(name, val);
                        } else if is_vars_command(line) {
                            println!("{}", format_vars(&vars));
                        } else if is_command(line) {
                            eprintln!("unknown command: {}", line.trim());
                        }
                        // Non-command lines in a command buffer are ignored
                    }
                    continue;
                }

                // Substitute variables, select request under cursor
                let cursor = cursor_pos.load(Ordering::Relaxed);
                match request_to_run(&buffer, cursor, &vars) {
                    Ok(Some(req)) => {
                        let mut out = stdout();
                        match execute(conn, &req) {
                            Ok(resp) => print_response(&mut out, &resp),
                            Err(e) => eprintln!("error: {}", e),
                        }
                        let _ = out.flush();
                    }
                    Ok(None) => {
                        eprintln!("no request found");
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
    fn enter_accepts_open_menu_before_inserting_newline() {
        let event = keybindings().find_binding(KeyModifiers::NONE, KeyCode::Enter);
        assert_eq!(
            event,
            Some(ReedlineEvent::UntilFound(vec![
                ReedlineEvent::MenuAccept,
                ReedlineEvent::Edit(vec![EditCommand::InsertNewline]),
            ]))
        );
    }

    #[test]
    fn cursor_offset_refers_to_unsubstituted_buffer() {
        let vars = HashMap::from([("i".to_string(), "a-very-long-index-name".to_string())]);
        // Offset 13 is inside "GET /b" in the buffer as typed
        let req = request_to_run("GET /${i}\n\nGET /b\n", 13, &vars).unwrap().unwrap();
        assert_eq!(req.path, "/b");
    }

    #[test]
    fn shift_tab_cycles_menu_backwards() {
        let event = keybindings().find_binding(KeyModifiers::SHIFT, KeyCode::BackTab);
        assert_eq!(event, Some(ReedlineEvent::MenuPrevious));
    }

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
