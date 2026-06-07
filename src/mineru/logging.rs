// SPDX-License-Identifier: MIT OR Apache-2.0

//! MinerU per-paper logging.
use std::collections::HashMap;
use std::io::Write;
use std::sync::Mutex;

static MINERU_LOGS: Mutex<Option<HashMap<String, std::fs::File>>> = Mutex::new(None);

tokio::task_local! {
    static CURRENT_PAPER_ID: String;
}

fn get_mineru_logs() -> &'static Mutex<Option<HashMap<String, std::fs::File>>> {
    &MINERU_LOGS
}

pub async fn with_paper_id<F>(id: &str, f: F) -> F::Output
where
    F: std::future::Future,
{
    open_mineru_log(id);
    let result = CURRENT_PAPER_ID.scope(id.to_string(), f).await;
    close_mineru_log(id);
    result
}

pub fn open_mineru_log(paper_id: &str) {
    if let Ok(mut logs_guard) = get_mineru_logs().lock() {
        let logs = logs_guard.get_or_insert_with(HashMap::new);
        let path = format!("figures/{}/mineru.log", paper_id);
        if let Ok(file) = std::fs::File::create(&path) {
            logs.insert(paper_id.to_string(), file);
        }
    }
}

pub fn close_mineru_log(paper_id: &str) {
    if let Ok(mut logs_guard) = get_mineru_logs().lock() {
        if let Some(logs) = logs_guard.as_mut() {
            logs.remove(paper_id);
        }
    }
}

pub(crate) fn pp_info(msg: &str) {
    tracing::info!("{}", msg);
    let _ = CURRENT_PAPER_ID.try_with(|pid| {
        if let Ok(logs_guard) = get_mineru_logs().lock() {
            if let Some(logs) = logs_guard.as_ref() {
                if let Some(mut file) = logs.get(pid) {
                    let _ = writeln!(file, "[INFO] {}", msg);
                }
            }
        }
    });
}

pub(crate) fn pp_warn(msg: &str) {
    tracing::warn!("{}", msg);
    let _ = CURRENT_PAPER_ID.try_with(|pid| {
        if let Ok(logs_guard) = get_mineru_logs().lock() {
            if let Some(logs) = logs_guard.as_ref() {
                if let Some(mut file) = logs.get(pid) {
                    let _ = writeln!(file, "[WARN] {}", msg);
                }
            }
        }
    });
}

pub(crate) fn pp_debug(msg: &str) {
    tracing::debug!("{}", msg);
    let _ = CURRENT_PAPER_ID.try_with(|pid| {
        if let Ok(logs_guard) = get_mineru_logs().lock() {
            if let Some(logs) = logs_guard.as_ref() {
                if let Some(mut file) = logs.get(pid) {
                    let _ = writeln!(file, "[DEBUG] {}", msg);
                }
            }
        }
    });
}
