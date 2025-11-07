use std::{error::Error, io::Cursor, sync::{atomic::AtomicBool, Arc}, time::Duration};

use tiny_http::{Response, Server};
use url::Url;

use crate::{constants, models::{FinishedAuthorization, PendingAuthorization}};

#[derive(thiserror::Error, Debug)]
pub enum ProcessAuthorizationError {
    #[error("Unable to start http server")]
    StartServer(Box<dyn Error + Send + Sync + 'static>),
    #[error("An I/O error occurred")]
    IoError(#[from] std::io::Error),
    #[error("Server-side error")]
    ServersideError(String),
    #[error("The csrf token in the request didn't match the response")]
    CsrfMismatch,
    #[error("The response didn't include the code")]
    MissingCode,
    #[error("The login was cancelled")]
    Cancelled,
}

pub fn start_server(pending_authroization: PendingAuthorization, cancel: Arc<AtomicBool>) -> Result<FinishedAuthorization, ProcessAuthorizationError> {
    let server = Server::http(constants::SERVER_ADDRESS)
        .map_err(ProcessAuthorizationError::StartServer)?;
    
    loop {
        let request = server.recv_timeout(Duration::from_millis(50))?;
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break Err(ProcessAuthorizationError::Cancelled);
        }
        let Some(request) = request else {
            continue;
        };
        
        let url = Url::parse(&format!("{}{}", constants::REDIRECT_URL_BASE, request.url())).unwrap();
        let mut error = None;
        let mut error_description = None;
        let mut code = None;
        let mut state = None;
        
        for (key, value) in url.query_pairs() {
            match &*key {
                "error" => error = Some(value),
                "error_description" => error_description = Some(value),
                "code" => code = Some(value),
                "state" => state = Some(value),
                _ => {
                    eprintln!("Unknown parameter: {:?} => {:?}", key, value);
                }
            }
        }
        
        let request = if let Some(ref code) = code {
            let url = url.to_string().replace(&**code, "hidden");
            
            // Redirect user immediately with a 302 to prevent code from being shown/saved in browser history
            // This definitely isn't necessary, but hey--I'm paranoid
            let _ = request.respond(Response::new(
                tiny_http::StatusCode(302),
                vec![
                    tiny_http::Header::from_bytes(&b"Location"[..], url).unwrap()
                ],
                std::io::empty(),
                Some(0),
                None,
            ));
            server.recv_timeout(Duration::from_millis(250)).ok().flatten()
        } else {
            Some(request)
        };
        
        if let Some(error) = error {
            let full_error = if let Some(error_description) = error_description {
                respond(request, &format!("An error occurred: {}", &*error), &*error_description, true);
                format!("An error occurred: {}\n{}", error, error_description)
            } else {
                respond(request, &format!("An error occurred: {}", &*error), "", true);
                format!("An error occurred: {}", error)
            };
            return Err(ProcessAuthorizationError::ServersideError(full_error));
        }
        
        if let Some(state) = state {
            if &*state != pending_authroization.csrf_token.secret() {
                respond(request, "Error: CSRF Mismatch!", "Did you reload the tab instead of going through the proper authorization flow?", true);
                return Err(ProcessAuthorizationError::CsrfMismatch);
            }
        }
        
        let Some(code) = code else {  
            respond(request, "Error", "Missing required 'code' parameter", true);
            return Err(ProcessAuthorizationError::MissingCode);
        };
        
        respond(request, "Authorization complete", "You may now close this window", false);
        
        return Ok(FinishedAuthorization {
            pending: pending_authroization,
            code: code.to_string()
        });
    }
}

fn respond(request: Option<tiny_http::Request>, main: &str, secondary: &str, error: bool) {
    let Some(request) = request else {
        return;
    };
    
    let status_code = if error {
        tiny_http::StatusCode(200)
    } else {
        tiny_http::StatusCode(400)
    };
    let string = format!(include_str!("auth_page.html"), main, secondary);
    let string_length = string.len();
    let response = Response::new(
        status_code,
        vec![
            tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"text/html; charset=UTF-8"[..]).unwrap()
        ],
        Cursor::new(string.into_bytes()),
        Some(string_length),
        None
    );
    let _ = request.respond(response);
}
