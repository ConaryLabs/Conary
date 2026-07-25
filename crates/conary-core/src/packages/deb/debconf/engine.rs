// conary-core/src/packages/deb/debconf/engine.rs

//! Deterministic noninteractive execution of typed debconf commands.

use super::protocol::escape_return;
use super::{
    DebconfCommand, DebconfExecution, DebconfGoOutcome, DebconfPendingQuestion, DebconfProgress,
    DebconfProgressCommand, DebconfProgressOutcome, DebconfResponse, DebconfResultCode,
    DebconfSession, DebconfState, DebconfStateError, DebconfTemplateLoadErrorKind,
    DebconfTemplateLoader, parse_argv, parse_command,
};

#[derive(Debug)]
pub struct DebconfEngine<L> {
    loader: L,
}

impl<L> DebconfEngine<L> {
    pub fn new(loader: L) -> Self {
        Self { loader }
    }

    pub fn loader(&self) -> &L {
        &self.loader
    }

    pub fn loader_mut(&mut self) -> &mut L {
        &mut self.loader
    }

    pub fn into_loader(self) -> L {
        self.loader
    }
}

impl<L: DebconfTemplateLoader> DebconfEngine<L> {
    pub fn process_line(
        &mut self,
        state: &mut DebconfState,
        session: &mut DebconfSession,
        raw_line: &[u8],
    ) -> DebconfExecution {
        if session.stopped {
            return reply(
                DebconfResultCode::InternalError,
                b"confmodule session has stopped".to_vec(),
            );
        }
        let command = match parse_command(raw_line, session.has_capability(b"escape")) {
            Ok(command) => command,
            Err(error) => return DebconfExecution::Reply(error.response()),
        };
        self.execute(state, session, command)
    }

    /// Execute command bytes and their already-tokenized arguments directly.
    /// This is the native lifecycle bridge entrypoint; it never joins and
    /// reparses argv through the whitespace-delimited wire grammar.
    pub fn process_argv(
        &mut self,
        state: &mut DebconfState,
        session: &mut DebconfSession,
        command: &[u8],
        arguments: &[Vec<u8>],
    ) -> DebconfExecution {
        if session.stopped {
            return reply(
                DebconfResultCode::InternalError,
                b"confmodule session has stopped".to_vec(),
            );
        }
        let command = match parse_argv(command, arguments) {
            Ok(command) => command,
            Err(error) => return DebconfExecution::Reply(error.response()),
        };
        self.execute(state, session, command)
    }

    pub fn execute(
        &mut self,
        state: &mut DebconfState,
        session: &mut DebconfSession,
        command: DebconfCommand,
    ) -> DebconfExecution {
        match command {
            DebconfCommand::Version { version } => {
                if let Some(version) = version {
                    if version.major < 2 {
                        return reply(
                            DebconfResultCode::CommandSpecific,
                            format!(
                                "Version too low ({})",
                                String::from_utf8_lossy(&version.wire_bytes())
                            )
                            .into_bytes(),
                        );
                    }
                    if version.major > 2 {
                        return reply(
                            DebconfResultCode::CommandSpecific,
                            format!(
                                "Version too high ({})",
                                String::from_utf8_lossy(&version.wire_bytes())
                            )
                            .into_bytes(),
                        );
                    }
                }
                success(super::DebconfProtocolVersion::CURRENT.wire_bytes())
            }
            DebconfCommand::Capb { capabilities } => {
                session.capabilities = capabilities;
                success(b"multiselect escape".to_vec())
            }
            DebconfCommand::Title { title } => {
                session.title = title;
                success(Vec::new())
            }
            DebconfCommand::SetTitle { question } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                if !state.questions.contains_key(&question) {
                    return missing_quoted(&question);
                }
                session.title = state
                    .expanded_field(&question, "description")
                    .unwrap_or_default();
                success(Vec::new())
            }
            DebconfCommand::Input { priority, question } => {
                self.input(state, session, priority, &question)
            }
            DebconfCommand::Go => self.go(state, session),
            DebconfCommand::Clear => {
                session.clear_pending();
                success(Vec::new())
            }
            DebconfCommand::BeginBlock => {
                session.begin_block();
                success(Vec::new())
            }
            DebconfCommand::EndBlock => {
                if session.end_block() {
                    success(Vec::new())
                } else {
                    reply(
                        DebconfResultCode::SyntaxError,
                        b"ENDBLOCK has no matching BEGINBLOCK".to_vec(),
                    )
                }
            }
            DebconfCommand::Stop => {
                session.finish(state);
                DebconfExecution::Stopped
            }
            DebconfCommand::Get { question } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                match state.effective_value(&question) {
                    Ok(value) => data_response(session, value),
                    Err(DebconfStateError::MissingQuestion(_)) => missing(&question),
                    Err(error) => internal(error),
                }
            }
            DebconfCommand::Set { question, value } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let Some(question) = state.questions.get_mut(&question) else {
                    return missing(&question);
                };
                question.value = Some(value);
                success(b"value set".to_vec())
            }
            DebconfCommand::Reset { question } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let default = match state.questions.get(&question) {
                    Some(question) => state
                        .templates
                        .get(&question.template)
                        .map(|template| template.default_value().to_vec()),
                    None => return missing(&question),
                };
                let Some(default) = default else {
                    return internal(DebconfStateError::MissingTemplate(question));
                };
                let question = state
                    .questions
                    .get_mut(&question)
                    .expect("question checked above");
                question.value = Some(default);
                question.seen = false;
                success(Vec::new())
            }
            DebconfCommand::Subst {
                question,
                key,
                value,
            } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let key = match identifier(&key, "substitution key") {
                    Ok(key) => key,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let Some(question) = state.questions.get_mut(&question) else {
                    return missing(&question);
                };
                question.substitutions.insert(key, value);
                success(Vec::new())
            }
            DebconfCommand::FGet { question, flag } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let flag = match identifier(&flag, "flag") {
                    Ok(flag) => flag,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let Some(question) = state.questions.get(&question) else {
                    return missing(&question);
                };
                success(
                    if question.flag(&flag) {
                        b"true".as_slice()
                    } else {
                        b"false".as_slice()
                    }
                    .to_vec(),
                )
            }
            DebconfCommand::FSet {
                question,
                flag,
                value,
            } => {
                let question_name = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let flag = match identifier(&flag, "flag") {
                    Ok(flag) => flag,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let Some(question) = state.questions.get_mut(&question_name) else {
                    return missing(&question_name);
                };
                question.set_flag(&flag, value.as_bool());
                if flag == "seen" {
                    session.seen.remove(&question_name);
                }
                success(value.as_bytes().to_vec())
            }
            DebconfCommand::MetaGet { question, field } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let field = match identifier(&field, "field") {
                    Ok(field) => field,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                if !state.questions.contains_key(&question) {
                    return missing(&question);
                }
                match state.metaget(&question, &field) {
                    Some(value) => data_response(session, value),
                    None => reply(
                        DebconfResultCode::InvalidParameters,
                        format!("{field} does not exist").into_bytes(),
                    ),
                }
            }
            DebconfCommand::Register { template, question } => {
                let template = match identifier(&template, "template") {
                    Ok(template) => template,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                match state.register(&template, &question, &session.owner) {
                    Ok(()) => success(Vec::new()),
                    Err(DebconfStateError::MissingTemplateQuestion(_)) => reply(
                        DebconfResultCode::InvalidParameters,
                        format!("No such template, \"{template}\"").into_bytes(),
                    ),
                    Err(error) => internal(error),
                }
            }
            DebconfCommand::Unregister { question } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                if !state.questions.contains_key(&question) {
                    return missing(&question);
                }
                if session.busy.contains(&question) {
                    return reply(
                        DebconfResultCode::InvalidParameters,
                        format!("{question} is busy, cannot unregister right now").into_bytes(),
                    );
                }
                state.remove_question_owner(&question, &session.owner);
                success(Vec::new())
            }
            DebconfCommand::Purge => {
                state.purge_owner(&session.owner);
                success(Vec::new())
            }
            DebconfCommand::Info { question } => {
                if let Some(question) = question {
                    let question = match identifier(&question, "question") {
                        Ok(question) => question,
                        Err(response) => return DebconfExecution::Reply(response),
                    };
                    if !state.questions.contains_key(&question) {
                        return missing_quoted(&question);
                    }
                    session.info = Some(question);
                } else {
                    session.info = None;
                }
                success(Vec::new())
            }
            DebconfCommand::Data {
                template,
                item,
                value,
            } => self.data(state, session, &template, &item, value),
            DebconfCommand::Progress(progress) => self.progress(state, session, progress),
            DebconfCommand::XLoadTemplateFile { path, owner } => {
                self.load_template_file(state, session, path, owner)
            }
            DebconfCommand::Visible {
                priority: _,
                question,
            } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                if !state.questions.contains_key(&question) {
                    return missing(&question);
                }
                success(b"false".to_vec())
            }
            DebconfCommand::Exist { question } => {
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                success(
                    if state.questions.contains_key(&question) {
                        b"true".as_slice()
                    } else {
                        b"false".as_slice()
                    }
                    .to_vec(),
                )
            }
        }
    }

    fn input(
        &mut self,
        state: &DebconfState,
        session: &mut DebconfSession,
        priority: super::DebconfPriority,
        raw_question: &[u8],
    ) -> DebconfExecution {
        let question_name = match identifier(raw_question, "question") {
            Ok(question) => question,
            Err(response) => return DebconfExecution::Reply(response),
        };
        let Some(question) = state.questions.get(&question_name) else {
            return missing_quoted(&question_name);
        };
        let template_type = match state.question_type(&question_name) {
            Ok(template_type) => template_type,
            Err(error) => return internal(error),
        };

        let eligible = if template_type == "error" {
            true
        } else {
            priority >= session.policy.priority && (session.policy.reshow || !question.seen)
        };
        let mark_seen = eligible && session.policy.noninteractive_seen;
        if noninteractive_element(template_type).is_none() {
            return skipped();
        }

        if !session
            .pending
            .iter()
            .any(|pending| pending.question == question_name)
        {
            session.pending.push(DebconfPendingQuestion {
                question: question_name.clone(),
                priority,
                visible: false,
                mark_seen,
                block_path: session.block_path(),
            });
        }
        session.busy.insert(question_name);
        skipped()
    }

    fn go(&mut self, state: &mut DebconfState, session: &mut DebconfSession) -> DebconfExecution {
        let requested = session.take_go_outcome();
        let has_visible = session.pending.iter().any(|pending| pending.visible);
        let proceed = requested == DebconfGoOutcome::Proceed && (!session.backed_up || has_visible);
        if !proceed {
            session.clear_pending();
            session.backed_up = true;
            return reply(DebconfResultCode::CommandSpecific, b"backup".to_vec());
        }

        let pending = session.pending.clone();
        for element in &pending {
            let value = match noninteractive_value(state, &element.question) {
                Ok(value) => value,
                Err(error) => {
                    session.clear_pending();
                    session.backed_up = false;
                    return internal(error);
                }
            };
            let Some(question) = state.questions.get_mut(&element.question) else {
                session.clear_pending();
                session.backed_up = false;
                return internal(DebconfStateError::MissingQuestion(element.question.clone()));
            };
            question.value = Some(value);
            if element.mark_seen {
                session.seen.insert(element.question.clone());
            }
        }
        session.clear_pending();
        session.backed_up = false;
        success(b"ok".to_vec())
    }

    fn data(
        &mut self,
        state: &mut DebconfState,
        session: &DebconfSession,
        raw_template: &[u8],
        raw_item: &[u8],
        value: Vec<u8>,
    ) -> DebconfExecution {
        let template_name = match identifier(raw_template, "template") {
            Ok(template) => template,
            Err(response) => return DebconfExecution::Reply(response),
        };
        let item = match identifier(raw_item, "template field") {
            Ok(item) => item,
            Err(response) => return DebconfExecution::Reply(response),
        };

        if !state.templates.contains_key(&template_name) {
            if item != "type" {
                return reply(
                    DebconfResultCode::InvalidParameters,
                    format!("Template data field '{item}' received before type field").into_bytes(),
                );
            }
            let template_type = match identifier(&value, "template type") {
                Ok(template_type) if !template_type.is_empty() => template_type,
                Ok(_) => {
                    return reply(
                        DebconfResultCode::InternalError,
                        b"Internal error making template".to_vec(),
                    );
                }
                Err(response) => return DebconfExecution::Reply(response),
            };
            return match state.create_data_template(&session.owner, template_name, template_type) {
                Ok(()) => success(Vec::new()),
                Err(error) => internal(error),
            };
        }
        if item == "type" {
            return reply(
                DebconfResultCode::InvalidParameters,
                b"Template type already set".to_vec(),
            );
        }
        state
            .templates
            .get_mut(&template_name)
            .expect("template checked above")
            .fields
            .insert(item, value);
        success(Vec::new())
    }

    fn progress(
        &mut self,
        state: &DebconfState,
        session: &mut DebconfSession,
        command: DebconfProgressCommand,
    ) -> DebconfExecution {
        match command {
            DebconfProgressCommand::Start {
                minimum,
                maximum,
                question,
            } => {
                if minimum > maximum {
                    return reply(
                        DebconfResultCode::SyntaxError,
                        format!("min ({minimum}) > max ({maximum})").into_bytes(),
                    );
                }
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                if !state.questions.contains_key(&question) {
                    return missing(&question);
                }
                session.progress = Some(DebconfProgress {
                    minimum,
                    maximum,
                    current: minimum,
                    question,
                    info: None,
                });
                success(b"OK".to_vec())
            }
            DebconfProgressCommand::Set { value } => {
                if session.progress.is_none() {
                    return no_progress();
                }
                if session.take_progress_outcome() == DebconfProgressOutcome::Cancel {
                    return canceled();
                }
                session.progress.as_mut().expect("checked above").current = value;
                success(b"OK".to_vec())
            }
            DebconfProgressCommand::Step { increment } => {
                let Some(current) = session.progress.as_ref().map(|progress| progress.current)
                else {
                    return no_progress();
                };
                if session.take_progress_outcome() == DebconfProgressOutcome::Cancel {
                    return canceled();
                }
                let Some(next) = current.checked_add(increment) else {
                    return reply(
                        DebconfResultCode::SyntaxError,
                        b"progress value overflow".to_vec(),
                    );
                };
                session.progress.as_mut().expect("checked above").current = next;
                success(b"OK".to_vec())
            }
            DebconfProgressCommand::Info { question } => {
                if session.progress.is_none() {
                    return no_progress();
                }
                let question = match identifier(&question, "question") {
                    Ok(question) => question,
                    Err(response) => return DebconfExecution::Reply(response),
                };
                if !state.questions.contains_key(&question) {
                    return missing(&question);
                }
                if session.take_progress_outcome() == DebconfProgressOutcome::Cancel {
                    return canceled();
                }
                session.progress.as_mut().expect("checked above").info = Some(question);
                success(b"OK".to_vec())
            }
            DebconfProgressCommand::Stop => {
                if session.progress.take().is_none() {
                    return no_progress();
                }
                success(b"OK".to_vec())
            }
        }
    }

    fn load_template_file(
        &mut self,
        state: &mut DebconfState,
        session: &DebconfSession,
        path: Vec<u8>,
        owner: Option<Vec<u8>>,
    ) -> DebconfExecution {
        let records = match self.loader.load(&path) {
            Ok(records) => records,
            Err(error) => {
                return match error.kind {
                    DebconfTemplateLoadErrorKind::Open => {
                        let mut message = b"failed to open ".to_vec();
                        message.extend_from_slice(&path);
                        message.extend_from_slice(b": ");
                        message.extend_from_slice(&error.message);
                        reply(DebconfResultCode::InvalidParameters, message)
                    }
                    DebconfTemplateLoadErrorKind::Parse => reply(
                        DebconfResultCode::InternalError,
                        quote_newlines(&error.message),
                    ),
                };
            }
        };
        let owner = match owner {
            Some(owner) => match identifier(&owner, "owner") {
                Ok(owner) => owner,
                Err(response) => return DebconfExecution::Reply(response),
            },
            None => session.owner.clone(),
        };
        match state.import_template_records(&owner, &records) {
            Ok(()) => success(Vec::new()),
            Err(error) => reply(
                DebconfResultCode::InternalError,
                quote_newlines(error.to_string().as_bytes()),
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum NoninteractiveElement {
    Base,
    Error,
    Select,
}

fn noninteractive_element(template_type: &str) -> Option<NoninteractiveElement> {
    let class = ucfirst(template_type);
    match class.as_str() {
        "Boolean" | "Multiselect" | "Note" | "Password" | "Progress" | "String" | "Text" => {
            Some(NoninteractiveElement::Base)
        }
        "Error" => Some(NoninteractiveElement::Error),
        "Select" => Some(NoninteractiveElement::Select),
        _ => None,
    }
}

fn ucfirst(value: &str) -> String {
    let mut characters = value.chars();
    let Some(first) = characters.next() else {
        return String::new();
    };
    first.to_uppercase().chain(characters).collect()
}

fn noninteractive_value(
    state: &DebconfState,
    question_name: &str,
) -> Result<Vec<u8>, DebconfStateError> {
    let template_type = state.question_type(question_name)?;
    match noninteractive_element(template_type) {
        Some(NoninteractiveElement::Error) => Ok(Vec::new()),
        Some(NoninteractiveElement::Select) => {
            let question = state
                .questions
                .get(question_name)
                .ok_or_else(|| DebconfStateError::MissingQuestion(question_name.to_string()))?;
            let template = state
                .templates
                .get(&question.template)
                .ok_or_else(|| DebconfStateError::MissingTemplate(question.template.clone()))?;
            let choice_field = if template.fields.contains_key("choices-c") {
                "choices-c"
            } else {
                "choices"
            };
            let choices = split_choices(
                &state
                    .expanded_field(question_name, choice_field)
                    .unwrap_or_default(),
            );
            let current = state.effective_value(question_name)?;
            if choices.iter().any(|choice| choice == &current) {
                Ok(current)
            } else {
                Ok(choices.into_iter().next().unwrap_or_default())
            }
        }
        Some(NoninteractiveElement::Base) => state.effective_value(question_name),
        None => Err(DebconfStateError::InvalidTemplateType(
            template_type.to_string(),
        )),
    }
}

fn split_choices(raw: &[u8]) -> Vec<Vec<u8>> {
    let mut choices = Vec::new();
    let mut choice = Vec::new();
    let mut index = 0;
    while index < raw.len() {
        if raw[index] == b'\\' && index + 1 < raw.len() && matches!(raw[index + 1], b',' | b' ') {
            choice.push(raw[index + 1]);
            index += 2;
        } else if raw[index] == b','
            && index + 1 < raw.len()
            && raw[index + 1].is_ascii_whitespace()
        {
            choices.push(std::mem::take(&mut choice));
            index += 2;
            while index < raw.len() && raw[index].is_ascii_whitespace() {
                index += 1;
            }
        } else {
            choice.push(raw[index]);
            index += 1;
        }
    }
    if !choice.is_empty() {
        choices.push(choice);
    }
    choices
}

fn identifier(raw: &[u8], kind: &str) -> Result<String, DebconfResponse> {
    std::str::from_utf8(raw).map(str::to_string).map_err(|_| {
        DebconfResponse::new(
            DebconfResultCode::SyntaxError,
            format!("{kind} is not valid UTF-8").into_bytes(),
        )
    })
}

fn data_response(session: &DebconfSession, value: Vec<u8>) -> DebconfExecution {
    if session.has_capability(b"escape") {
        reply(DebconfResultCode::EscapedData, escape_return(&value))
    } else {
        success(value)
    }
}

fn missing(name: &str) -> DebconfExecution {
    reply(
        DebconfResultCode::InvalidParameters,
        format!("{name} doesn't exist").into_bytes(),
    )
}

fn missing_quoted(name: &str) -> DebconfExecution {
    reply(
        DebconfResultCode::InvalidParameters,
        format!("\"{name}\" doesn't exist").into_bytes(),
    )
}

fn skipped() -> DebconfExecution {
    reply(
        DebconfResultCode::CommandSpecific,
        b"question skipped".to_vec(),
    )
}

fn canceled() -> DebconfExecution {
    reply(DebconfResultCode::CommandSpecific, b"CANCELED".to_vec())
}

fn no_progress() -> DebconfExecution {
    reply(
        DebconfResultCode::InvalidParameters,
        b"progress bar is not active".to_vec(),
    )
}

fn internal(error: impl std::fmt::Display) -> DebconfExecution {
    reply(
        DebconfResultCode::InternalError,
        error.to_string().into_bytes(),
    )
}

fn success(ret: Vec<u8>) -> DebconfExecution {
    DebconfExecution::Reply(DebconfResponse::success(ret))
}

fn reply(code: DebconfResultCode, ret: Vec<u8>) -> DebconfExecution {
    DebconfExecution::Reply(DebconfResponse::new(code, ret))
}

fn quote_newlines(raw: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(raw.len());
    for byte in raw {
        if *byte == b'\n' {
            output.extend_from_slice(b"\\n");
        } else {
            output.push(*byte);
        }
    }
    output
}
