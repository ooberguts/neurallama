//! Ready-to-paste client snippets.

use super::Snippets;

pub fn build(base_url_v1: &str, api_key: &str, model: &str) -> Snippets {
    let curl = format!(
        "curl {base_url_v1}/chat/completions ^\n  -H \"Content-Type: application/json\" ^\n  -H \"Authorization: Bearer {api_key}\" ^\n  -d \"{{\\\"model\\\": \\\"{model}\\\", \\\"messages\\\": [{{\\\"role\\\": \\\"user\\\", \\\"content\\\": \\\"Hello!\\\"}}]}}\""
    );
    let python = format!(
        r#"from openai import OpenAI

client = OpenAI(base_url="{base_url_v1}", api_key="{api_key}")
reply = client.chat.completions.create(
    model="{model}",
    messages=[{{"role": "user", "content": "Hello!"}}],
)
print(reply.choices[0].message.content)
"#
    );
    let node = format!(
        r#"import OpenAI from "openai";

const client = new OpenAI({{ baseURL: "{base_url_v1}", apiKey: "{api_key}" }});
const reply = await client.chat.completions.create({{
  model: "{model}",
  messages: [{{ role: "user", content: "Hello!" }}],
}});
console.log(reply.choices[0].message.content);
"#
    );
    Snippets { curl, python, node }
}

#[cfg(test)]
mod tests {
    #[test]
    fn snippets_embed_values() {
        let s = super::build("http://127.0.0.1:8080/v1", "nl-abc", "local");
        assert!(s.curl.contains("http://127.0.0.1:8080/v1/chat/completions"));
        assert!(s.curl.contains("Bearer nl-abc"));
        assert!(s.python.contains(r#"model="local""#));
        assert!(s.node.contains(r#"baseURL: "http://127.0.0.1:8080/v1""#));
    }
}
