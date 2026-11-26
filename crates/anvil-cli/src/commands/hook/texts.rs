//! User-facing `Stop` texts in the user's language.

use anvil_core::locale::Lang;

/// How far the pending change reaches.
pub fn impact(lang: Lang, files: usize, tests: usize) -> String {
    match lang {
        Lang::En => format!("This change reaches {files} file(s) and {tests} related test(s)."),
        Lang::Es => format!("El cambio alcanza {files} archivo(s) y {tests} test(s) relacionados."),
    }
}

pub fn ui_changed(lang: Lang) -> String {
    lang.pick(
        "UI files changed; check them with `anvil qa`.",
        "Cambiaron archivos de interfaz; revísalos con `anvil qa`.",
    )
    .to_owned()
}

/// The breaker is open, in `subrepo` when given.
pub fn circuit_open(lang: Lang, subrepo: Option<&str>) -> String {
    match (lang, subrepo) {
        (Lang::En, None) => "The circuit breaker is open after repeated identical failures; a person should look before work continues (`anvil circuit status`).".to_owned(),
        (Lang::En, Some(sub)) => format!("The circuit breaker is open in `{sub}` after repeated identical failures; a person should look before work continues (`anvil circuit status`)."),
        (Lang::Es, None) => "El circuit breaker está abierto tras fallos idénticos repetidos; una persona debe revisarlo antes de seguir (`anvil circuit status`).".to_owned(),
        (Lang::Es, Some(sub)) => format!("El circuit breaker está abierto en `{sub}` tras fallos idénticos repetidos; una persona debe revisarlo antes de seguir (`anvil circuit status`)."),
    }
}

/// Tier 0 found errors in the changed files, in `subrepo` when given.
pub fn gate_failed(lang: Lang, subrepo: Option<&str>, errors: &[String]) -> String {
    let count = errors.len();
    let list = errors.join("; ");
    let place = subrepo.map(|sub| format!(" (`{sub}`)")).unwrap_or_default();
    match lang {
        Lang::En => format!("Tier 0 check failed{place}, {count} error(s): {list}"),
        Lang::Es => format!("El check Tier 0 falló{place}, {count} error(es): {list}"),
    }
}

/// The active task was never closed or checkpointed.
pub fn task_in_progress(lang: Lang, id: &str) -> String {
    match lang {
        Lang::En => format!(
            "Task {id} is still in progress. Close it with MCP `task_done` / `anvil tasks done {id} \"<summary>\"`, or mark it blocked with `task_update` (`status=\"blocked\"`) / `anvil tasks update {id} \"<progress>\" --status blocked --next \"...\"`."
        ),
        Lang::Es => format!(
            "La tarea {id} sigue en curso. Ciérrala con MCP `task_done` / `anvil tasks done {id} \"<resumen>\"`, o márcala bloqueada con `task_update` (`status=\"blocked\"`) / `anvil tasks update {id} \"<avance>\" --status blocked --next \"...\"`."
        ),
    }
}

/// Substantive edits need a task decision; `last` is the latest task's id and request.
pub fn curation_prompt(lang: Lang, file_count: usize, last: Option<(&str, &str)>) -> String {
    match lang {
        Lang::En => {
            let reuse = last
                .map(|(id, request)| format!(" (2) If it continues `{id}` (\"{request}\"), append to it with MCP `task_done` / `anvil tasks done {id} \"<summary>\"`."))
                .unwrap_or_default();
            format!(
                "Edits across {file_count} file(s) need a task decision (never copy the user's chat as the title): (1) a real feature, refactor or fix -> MCP `task_record` or `anvil tasks record \"<Clean Engineering Title>\" \"<summary>\"`.{reuse} (3) a minor tweak or chore -> MCP `task_skip` or `anvil tasks skip \"<reason>\"`."
            )
        }
        Lang::Es => {
            let reuse = last
                .map(|(id, request)| format!(" (2) Si continúa `{id}` (\"{request}\"), agrégalo con MCP `task_done` / `anvil tasks done {id} \"<resumen>\"`."))
                .unwrap_or_default();
            format!(
                "Los cambios en {file_count} archivo(s) necesitan una decisión de tarea (nunca copies el chat del usuario como título): (1) una funcionalidad, refactor o corrección real -> MCP `task_record` o `anvil tasks record \"<Título técnico claro>\" \"<resumen>\"`.{reuse} (3) un ajuste menor -> MCP `task_skip` o `anvil tasks skip \"<motivo>\"`."
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spanish_and_english_differ_and_keep_the_numbers() {
        let es = impact(Lang::Es, 110, 33);
        let en = impact(Lang::En, 110, 33);
        assert!(es.contains("110 archivo(s)") && es.contains("33 test(s)"));
        assert!(en.contains("110 file(s)") && en.contains("33 related test(s)"));
    }

    #[test]
    fn task_text_names_the_task_in_both_languages() {
        assert!(task_in_progress(Lang::Es, "T-7").starts_with("La tarea T-7 sigue en curso"));
        assert!(task_in_progress(Lang::En, "T-7").starts_with("Task T-7 is still in progress"));
        assert!(gate_failed(Lang::Es, Some("api"), &["a".to_owned()]).contains("(`api`), 1 error(es): a"));
    }
}
