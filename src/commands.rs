//! The commands the server runs on `workspace/executeCommand`.
//!
//! [`COMMANDS`] is what the server advertises; [`parse`] turns a request into a
//! [`Command`], and the server performs its effect.

use crate::imports::FETCH_COMMAND;
use lsp_server::ErrorCode;
use lsp_types::ExecuteCommandParams;
use serde_json::Value;

/// Run the SPARQL query of a document over the workspace (the "▶ Run query"
/// lens), with the document's URI.
pub const RUN_QUERY: &str = "turtle.runQuery";

/// Every command name the server advertises.
pub const COMMANDS: &[&str] = &[FETCH_COMMAND, RUN_QUERY];

/// A parsed `workspace/executeCommand` request.
#[derive(Debug, PartialEq, Eq)]
pub enum Command {
    /// Fetch one `owl:imports` target from the network.
    FetchImport { iri: String },
    /// Run the query of one SPARQL document.
    RunQuery { uri: String },
}

/// Parses `params`; bad arguments and unknown commands are `InvalidParams`.
pub fn parse(params: ExecuteCommandParams) -> Result<Command, (ErrorCode, String)> {
    let ExecuteCommandParams {
        command,
        mut arguments,
        ..
    } = params;
    match (command.as_str(), arguments.as_mut_slice()) {
        (FETCH_COMMAND, [Value::String(iri)]) => Ok(Command::FetchImport {
            iri: std::mem::take(iri),
        }),
        (FETCH_COMMAND, _) => Err((ErrorCode::InvalidParams, format!("{command} takes one IRI"))),
        (RUN_QUERY, [Value::String(uri)]) => Ok(Command::RunQuery {
            uri: std::mem::take(uri),
        }),
        (RUN_QUERY, _) => Err((
            ErrorCode::InvalidParams,
            format!("{command} takes one document URI"),
        )),
        _ => Err((
            ErrorCode::InvalidParams,
            format!("unknown command {command}"),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn params(command: &str, arguments: Vec<Value>) -> ExecuteCommandParams {
        ExecuteCommandParams {
            command: command.into(),
            arguments,
            work_done_progress_params: Default::default(),
        }
    }

    #[test]
    fn fetch_import_with_one_iri() {
        let p = params(FETCH_COMMAND, vec![json!("http://example.org/o")]);
        let Ok(command) = parse(p) else {
            panic!("rejected")
        };
        assert_eq!(
            command,
            Command::FetchImport {
                iri: "http://example.org/o".into()
            }
        );
    }

    #[test]
    fn fetch_import_with_wrong_arguments() {
        for args in [
            vec![],
            vec![json!("http://a/"), json!("http://b/")],
            vec![json!(1)],
        ] {
            let Err((code, msg)) = parse(params(FETCH_COMMAND, args)) else {
                panic!("accepted")
            };
            assert_eq!(code as i32, ErrorCode::InvalidParams as i32);
            assert_eq!(msg, "turtle.fetchImport takes one IRI");
        }
    }

    #[test]
    fn run_query_with_one_uri() {
        let Ok(command) = parse(params(RUN_QUERY, vec![json!("file:///w/q.rq")])) else {
            panic!("rejected")
        };
        assert_eq!(
            command,
            Command::RunQuery {
                uri: "file:///w/q.rq".into()
            }
        );
        let Err((code, msg)) = parse(params(RUN_QUERY, vec![])) else {
            panic!("accepted")
        };
        assert_eq!(code as i32, ErrorCode::InvalidParams as i32);
        assert_eq!(msg, "turtle.runQuery takes one document URI");
        assert!(COMMANDS.contains(&RUN_QUERY));
    }

    #[test]
    fn unknown_command() {
        let Err((code, msg)) = parse(params("turtle.nope", vec![])) else {
            panic!("accepted")
        };
        assert_eq!(code as i32, ErrorCode::InvalidParams as i32);
        assert_eq!(msg, "unknown command turtle.nope");
    }
}
