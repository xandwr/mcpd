use crate::mcp::Tool;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindToolsParams {
    #[serde(default)]
    pub query: String,
    pub server: Option<String>,
    #[serde(default = "default_limit")]
    pub limit: usize,
}

fn default_limit() -> usize {
    10
}

impl FindToolsParams {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=100).contains(&self.limit) {
            return Err("limit must be between 1 and 100".into());
        }
        Ok(())
    }
}

#[derive(Debug, Serialize)]
pub struct FoundTool {
    pub server: String,
    pub name: String,
    pub description: String,
    pub input_schema: serde_json::Value,
}

#[derive(Debug, Serialize)]
pub struct BackendError {
    pub server: String,
    pub error: String,
}

#[derive(Debug, Serialize)]
pub struct FindToolsResult {
    pub tools: Vec<FoundTool>,
    pub total_matches: usize,
    pub servers: Vec<String>,
    pub errors: Vec<BackendError>,
}

pub fn rank_tools(
    params: &FindToolsParams,
    catalogs: Vec<(String, Vec<Tool>)>,
    mut servers: Vec<String>,
    mut errors: Vec<BackendError>,
) -> FindToolsResult {
    let query = params.query.to_lowercase();
    let mut terms: Vec<_> = query.split_whitespace().collect();
    terms.sort_unstable();
    terms.dedup();
    let mut matches = Vec::new();
    for (server, tools) in catalogs {
        for tool in tools {
            let name = format!("{}__{}", server, tool.name);
            let normalized_name = name.to_lowercase();
            let description = tool.description.unwrap_or_default();
            let normalized_description = description.to_lowercase();
            let score: usize = terms
                .iter()
                .map(|term| {
                    if normalized_name.contains(term) {
                        3
                    } else if normalized_description.contains(term) {
                        1
                    } else {
                        0
                    }
                })
                .sum();
            if terms.is_empty() || score > 0 {
                matches.push((
                    score,
                    FoundTool {
                        server: server.clone(),
                        name,
                        description,
                        input_schema: tool.input_schema,
                    },
                ));
            }
        }
    }
    matches.sort_by(|(a_score, a), (b_score, b)| {
        b_score.cmp(a_score).then_with(|| a.name.cmp(&b.name))
    });
    let total_matches = matches.len();
    servers.sort();
    errors.sort_by(|a, b| a.server.cmp(&b.server));
    FindToolsResult {
        tools: matches
            .into_iter()
            .take(params.limit)
            .map(|(_, tool)| tool)
            .collect(),
        total_matches,
        servers,
        errors,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn search_ranks_names_limits_results_and_preserves_schemas() {
        let params: FindToolsParams = serde_json::from_value(json!({
            "query": "ECHO echo", "limit": 1
        }))
        .unwrap();
        let result = rank_tools(
            &params,
            vec![(
                "mock".into(),
                vec![
                    Tool {
                        name: "describe".into(),
                        description: Some("Echo something".into()),
                        input_schema: json!({}),
                    },
                    Tool {
                        name: "echo".into(),
                        description: None,
                        input_schema: json!({"type": "object"}),
                    },
                    Tool {
                        name: "fail".into(),
                        description: None,
                        input_schema: json!({}),
                    },
                ],
            )],
            vec!["mock".into()],
            vec![],
        );
        assert_eq!(result.total_matches, 2);
        assert_eq!(result.tools.len(), 1);
        assert_eq!(result.tools[0].name, "mock__echo");
        assert_eq!(result.tools[0].input_schema, json!({"type": "object"}));
    }

    #[test]
    fn browse_is_sorted_and_server_names_are_searchable() {
        for query in ["  ", "MOCK"] {
            let params = serde_json::from_value(json!({"query": query})).unwrap();
            let result = rank_tools(
                &params,
                vec![(
                    "mock".into(),
                    vec![
                        Tool {
                            name: "z".into(),
                            description: None,
                            input_schema: json!({}),
                        },
                        Tool {
                            name: "a".into(),
                            description: None,
                            input_schema: json!({}),
                        },
                    ],
                )],
                vec!["z".into(), "mock".into()],
                vec![],
            );
            assert_eq!(result.tools[0].name, "mock__a");
            assert_eq!(result.total_matches, 2);
            assert_eq!(result.servers, vec!["mock", "z"]);
        }
    }
}
