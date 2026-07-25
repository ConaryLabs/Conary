// apps/conary/src/commands/repo_static/test_support.rs

#[derive(Clone)]
struct PromptOverride {
    interactive: bool,
    accept: bool,
    prompt: Option<String>,
}

thread_local! {
    static PROMPT_OVERRIDE: std::cell::RefCell<Option<PromptOverride>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) async fn with_static_repo_prompt_override<F, Fut, T>(
    interactive: bool,
    accept: bool,
    f: F,
) -> (T, Option<String>)
where
    F: FnOnce() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    PROMPT_OVERRIDE.with(|cell| {
        *cell.borrow_mut() = Some(PromptOverride {
            interactive,
            accept,
            prompt: None,
        });
    });

    let output = f().await;
    let prompt =
        PROMPT_OVERRIDE.with(|cell| cell.borrow_mut().take().and_then(|state| state.prompt));
    (output, prompt)
}

pub(super) fn test_prompt_interactive_override() -> Option<bool> {
    PROMPT_OVERRIDE.with(|cell| cell.borrow().as_ref().map(|state| state.interactive))
}

pub(super) fn record_test_prompt(prompt: &str) -> Option<bool> {
    PROMPT_OVERRIDE.with(|cell| {
        let mut state = cell.borrow_mut();
        let state = state.as_mut()?;
        state.prompt = Some(prompt.to_string());
        Some(state.accept)
    })
}
