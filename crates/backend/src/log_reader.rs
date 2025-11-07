use std::{borrow::Cow, io::{BufRead, BufReader, Read}, process::ChildStdout, sync::{atomic::AtomicUsize, Arc}};

use bridge::{game_output::GameOutputLogLevel, handle::FrontendHandle, keep_alive::KeepAlive, message::MessageToFrontend};
use chrono::Utc;

static GAME_OUTPUT_ID: AtomicUsize = AtomicUsize::new(0);

pub fn start_game_output(stdout: ChildStdout, sender: FrontendHandle) {
    let unknown_thread: Arc<str> = Arc::from("<unknown thread>");
    let empty_message: Arc<str> = Arc::from("<empty>");
    
    std::thread::spawn(move || {
        let id = GAME_OUTPUT_ID.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        
        let keep_alive = KeepAlive::new();
        let keep_alive_handle = keep_alive.create_handle();
        let _ = sender.blocking_send(MessageToFrontend::CreateGameOutputWindow {
            id,
            keep_alive
        });
        
        let mut raw_reader = BufReader::new(stdout);
        let mut reader = quick_xml::reader::Reader::from_reader(raw_reader);
        
        let mut buf = Vec::new();
        let mut stack = Vec::new();
        
        #[derive(Debug)]
        enum ParseState {
            Event {
                timestamp: i64,
                thread: Arc<str>,
                level: GameOutputLogLevel,
                text: Option<Arc<str>>,
                throwable: Option<Arc<str>>,
            },
            Message {
                content: Option<Arc<str>>,
            },
            Throwable {
                content: Option<Arc<str>>,
            },
            Unknown,
        }
        
        let mut last_thread: Option<Arc<str>> = None;
        let mut last_message: Option<Arc<str>> = None;
        let mut last_throwable: Option<Arc<str>> = None;
        
        let mut raw_text = String::new();
        let mut read_raw_text = false;
        
        let replacements = [
            // Access token replacements
            (regex::Regex::new(r#"SignedJWT: [^\s]+"#).unwrap(), "SignedJWT: *****"),
            (regex::Regex::new(r#"Session ID is token: [^\s)]+"#).unwrap(), "Session ID is token: *****"),
            (regex::Regex::new(r#"--accessToken [^\s]+"#).unwrap(), "--accessToken *****"),
            // Computer username replacements
            (regex::Regex::new(r#"\/home\/[^/]+\/"#).unwrap(), "/home/*****/"),
            (regex::Regex::new(r#"\/Users\/[^/]+\/"#).unwrap(), "/Users/*****/"),
            (regex::Regex::new(r#"\\Users\\[^\\]+\\"#).unwrap(), "\\Users\\*****\\"),
            (regex::Regex::new(r#"\\\\Users\\\\[^/]+\\\\"#).unwrap(), "\\\\Users\\\\*****\\\\"),
        ];
        
        while keep_alive_handle.is_alive() {
            buf.clear();
            match reader.read_event_into(&mut buf) {
                Err(e) => panic!("Error at position {}: {:?}", reader.error_position(), e),
                Ok(quick_xml::events::Event::Eof) => {
                    _ = sender.blocking_send(MessageToFrontend::AddGameOutput {
                        id,
                        time: chrono::Utc::now().timestamp_millis(),
                        thread: Arc::from("main"),
                        level: GameOutputLogLevel::Info,
                        text: Arc::new([Arc::from("<end of output>")]),
                    });
                    break;
                }
                Ok(quick_xml::events::Event::Start(e)) => {
                    match stack.last_mut() {
                        None => {
                            match e.name().as_ref() {
                                b"log4j:Event" => {
                                    let mut timestamp = 0;
                                    let mut thread = unknown_thread.clone();
                                    let mut level = GameOutputLogLevel::Other;
                                    for attribute in e.attributes() {
                                        let Ok(attribute) = attribute else {
                                            continue;
                                        };
                                        let key = attribute.key.as_ref();
                                        match key {
                                            b"timestamp" => {
                                                let Ok(value) = str::from_utf8(&*attribute.value) else {
                                                    continue;
                                                };
                                                if let Ok(parsed) = value.parse() {
                                                    timestamp = parsed;
                                                }
                                            },
                                            b"level" => {
                                                level = match &*attribute.value {
                                                    b"FATAL" => GameOutputLogLevel::Fatal,
                                                    b"ERROR" => GameOutputLogLevel::Error,
                                                    b"WARN" => GameOutputLogLevel::Warn,
                                                    b"INFO" => GameOutputLogLevel::Info,
                                                    b"DEBUG" => GameOutputLogLevel::Debug,
                                                    b"TRACE" => GameOutputLogLevel::Trace,
                                                    _ => GameOutputLogLevel::Other,
                                                };
                                            },
                                            b"thread" => {
                                                // Try to reuse last thread to avoid duplicate string allocations
                                                if let Some(last_thread) = &last_thread {
                                                    if last_thread.as_bytes() == &*attribute.value {
                                                        thread = last_thread.clone();
                                                        continue;
                                                    }
                                                }
                                                
                                                let Ok(value) = str::from_utf8(&*attribute.value) else {
                                                    continue;
                                                };
                                                thread = Arc::from(value);
                                                last_thread = Some(thread.clone());
                                            },
                                            b"logger" => {},
                                            _ => {
                                                if cfg!(debug_assertions) {
                                                    panic!("Unknown attribute on log4j:Event: {:?}", String::from_utf8_lossy(key))
                                                }
                                            }
                                        }
                                    }
                                    stack.push(ParseState::Event { timestamp, thread, level, text: None, throwable: None });
                                }
                                _ => {
                                    if cfg!(debug_assertions) {
                                        panic!("Unknown tag {:?} for stack {:?}", e.name(), &stack);
                                    }
                                    stack.push(ParseState::Unknown);
                                }
                            }
                        },
                        Some(ParseState::Event { .. }) => {
                            match e.name().as_ref() {
                                b"log4j:Message" => {
                                    stack.push(ParseState::Message { content: None });
                                },
                                b"log4j:Throwable" => {
                                    stack.push(ParseState::Throwable { content: None });
                                },
                                _ => {
                                    if cfg!(debug_assertions) {
                                        panic!("Unknown tag {:?} for stack {:?}", e.name(), &stack);
                                    }
                                    stack.push(ParseState::Unknown);
                                }
                            }
                        },
                        Some(ParseState::Unknown) => {
                            stack.push(ParseState::Unknown);
                        }
                        _ => {}
                    }
                },
                Ok(quick_xml::events::Event::End(_)) => {
                    let Some(popped) = stack.pop() else {
                        if cfg!(debug_assertions) {
                            panic!("End called when stack was empty!?");
                        }
                        continue;
                    };
                    match stack.last_mut() {
                        None => {
                            if let ParseState::Event { timestamp, thread, level, mut text, mut throwable  } = popped {
                                let mut lines = Vec::new();
                                
                                if let Some(text) = text.as_mut() {
                                    let mut replaced = Cow::Borrowed(&**text);
                                    for (regex, replacement) in &replacements {
                                        if let Cow::Owned(new) = regex.replace_all(&*replaced, *replacement) {
                                            replaced = Cow::Owned(new);
                                        }
                                    }
                                    if let Cow::Owned(replaced) = replaced {
                                        *text = replaced.into();
                                    }
                                }
                                if let Some(throwable) = throwable.as_mut() {
                                    let mut replaced = Cow::Borrowed(&**throwable);
                                    for (regex, replacement) in &replacements {
                                        if let Cow::Owned(new) = regex.replace_all(&*replaced, *replacement) {
                                            replaced = Cow::Owned(new);
                                        }
                                    }
                                    if let Cow::Owned(replaced) = replaced {
                                        *throwable = replaced.into();
                                    }
                                }
                                
                                if let Some(text) = &text {
                                    let mut split = text.trim_end().split("\n");
                                    if let Some(first) = split.next() && let Some(second) = split.next() {
                                        lines.push(Arc::from(first.trim_end()));
                                        lines.push(Arc::from(second.trim_end()));
                                        while let Some(next) = split.next() {
                                            lines.push(Arc::from(next.trim_end()));
                                        }
                                    }
                                }
                                if let Some(throwable) = &throwable {
                                    let mut split = throwable.trim_end().split("\n");
                                    if let Some(first) = split.next() && let Some(second) = split.next() {
                                        if let Some(text) = text.take() && lines.is_empty() {
                                            lines.push(text);
                                        }
                                        
                                        lines.push(Arc::from(first.trim_end()));
                                        lines.push(Arc::from(second.trim_end()));
                                        while let Some(next) = split.next() {
                                            lines.push(Arc::from(next.trim_end()));
                                        }
                                    }
                                }
                                
                                let final_lines: Arc<[Arc<str>]> = if !lines.is_empty() {
                                    lines.into()
                                } else if let Some(text) = text.take() {
                                    if let Some(throwable) = throwable.take() {
                                        Arc::new([text, throwable])
                                    } else {
                                        Arc::new([text])
                                    }
                                } else {
                                    if let Some(throwable) = throwable {
                                        Arc::new([throwable])
                                    } else {
                                        Arc::new([empty_message.clone()])
                                    }
                                };
                                _ = sender.blocking_send(MessageToFrontend::AddGameOutput {
                                    id,
                                    time: timestamp,
                                    thread,
                                    level,
                                    text: final_lines,
                                });
                            } else if cfg!(debug_assertions) {
                                panic!("Don't know how to handle popping {:?} on root", popped);
                            }
                        }
                        Some(ParseState::Event { text, throwable, .. }) => {
                            if let ParseState::Message { content } = popped {
                                *text = content;
                            } else if let ParseState::Throwable { content } = popped {
                                *throwable = content;
                            } else if cfg!(debug_assertions) {
                                panic!("Don't know how to handle popping {:?} on Event", popped);
                            }
                        }
                        last => {
                            if cfg!(debug_assertions) {
                                panic!("Don't know how to handle popping {:?} on {:?}", popped, last);
                            }
                        }
                    }
                    
                },
                Ok(quick_xml::events::Event::CData(e)) => {
                    match stack.last_mut() {
                        Some(ParseState::Message { content, .. }) => {
                            // Try to reuse last message to avoid duplicate string allocations
                            if let Some(last_message) = &last_message {
                                if last_message.as_bytes() == &*e {
                                    *content = Some(last_message.clone());
                                    continue;
                                }
                            }
                            
                            let message: Arc<str> = String::from_utf8_lossy(&*e).into_owned().into();
                            *content = Some(message.clone());
                            last_message = Some(message);
                        }
                        Some(ParseState::Throwable { content, .. }) => {
                            // Try to reuse last throwable to avoid duplicate string allocations
                            if let Some(last_throwable) = &last_throwable {
                                if last_throwable.as_bytes() == &*e {
                                    *content = Some(last_throwable.clone());
                                    continue;
                                }
                            }
                            
                            let message: Arc<str> = String::from_utf8_lossy(&*e).into_owned().into();
                            *content = Some(message.clone());
                            last_throwable = Some(message);
                        }
                        last => {
                            if cfg!(debug_assertions) {
                                panic!("Don't know how to handle cdata on {:?}", last);
                            }
                        }
                    }
                },
                Ok(quick_xml::events::Event::Text(e)) => {
                    let read_raw = String::from_utf8_lossy(&*e);
                    if read_raw.trim_ascii().is_empty() {
                        continue;
                    }
                    
                    if stack.is_empty() {
                        // We got text at the root level, fallback to writing raw text output
                        read_raw_text = true;
                        raw_text.push_str(&*read_raw);
                        if reader.buffer_position()+1 == reader.stream().offset() {
                            raw_text.push('<');
                        } else {
                            debug_assert_eq!(reader.buffer_position(), reader.stream().offset());
                        }
                        break;
                    } else if cfg!(debug_assertions) {
                        panic!("Don't know how to handle raw text on {:?}", stack.last());
                    }
                },
                Ok(e) => {
                    if cfg!(debug_assertions) {
                        panic!("Unknown event {:?}", e);
                    }
                }
            }
        }
        if read_raw_text {
            let level = if raw_text.contains("Minecraft Crash Report") {
                GameOutputLogLevel::Fatal
            } else {
                GameOutputLogLevel::Info
            };
            
            let mut last: Option<&str> = None;
            for split in raw_text.split("\n") {
                if let Some(last) = last {
                    _ = sender.blocking_send(MessageToFrontend::AddGameOutput {
                        id,
                        time: Utc::now().timestamp_millis(),
                        thread: unknown_thread.clone(),
                        level,
                        text: Arc::new([last.trim_end().into()]),
                    });
                }
                last = Some(split);
            }
            if let Some(last) = last {
                raw_text = last.to_string();
            } else {
                raw_text.clear();
            }
            
            let mut stream = reader.stream();
            let stream = stream.get_mut();
            while keep_alive_handle.is_alive() {
                match stream.read_line(&mut raw_text) {
                    Err(e) => panic!("Error while reading raw: {:?}", e),
                    Ok(0) => {
                        break; // EOF
                    },
                    Ok(_) => {
                        let mut replaced = Cow::Borrowed(&*raw_text);
                        for (regex, replacement) in &replacements {
                            if let Cow::Owned(new) = regex.replace_all(&*replaced, *replacement) {
                                replaced = Cow::Owned(new);
                            }
                        }
                        
                        _ = sender.blocking_send(MessageToFrontend::AddGameOutput {
                            id,
                            time: Utc::now().timestamp_millis(),
                            thread: unknown_thread.clone(),
                            level,
                            text: Arc::new([replaced.trim_end().into()]),
                        });
                        raw_text.clear();
                    }
                }
            }
        }
    });
}
