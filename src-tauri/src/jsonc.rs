use serde_json::{Map, Value};
use std::ops::Range;

const INVALID: &str = "本机配置必须是有效的 JSON 或 JSONC 对象";

pub struct Document(String);

pub fn document(bytes: Option<&[u8]>) -> Result<Document, String> {
    let text = std::str::from_utf8(bytes.unwrap_or(b"{}\n")).map_err(|_| INVALID)?;
    Ok(Document(text.to_owned()))
}

struct Token {
    kind: u8,
    range: Range<usize>,
}

// Mask comments and trailing commas without moving the ranges used for edits.
fn tokens(text: &str) -> Result<(Vec<Token>, Vec<u8>), String> {
    let source = text.as_bytes();
    let mut plain = source.to_vec();
    let mut tokens: Vec<Token> = Vec::new();
    let mut at = if source.starts_with(b"\xef\xbb\xbf") {
        plain[..3].fill(b' ');
        3
    } else {
        0
    };
    while let Some(&kind) = source.get(at) {
        let start = at;
        at += 1;
        match kind {
            b' ' | b'\r' | b'\n' | b'\t' => continue,
            b'/' => {
                at = match source.get(at) {
                    Some(b'/') => source[at..]
                        .iter()
                        .position(|byte| matches!(byte, b'\r' | b'\n'))
                        .map_or(source.len(), |length| at + length),
                    Some(b'*') => memchr::memmem::find(&source[at + 1..], b"*/")
                        .map(|length| at + 1 + length + 2)
                        .ok_or(INVALID)?,
                    _ => return Err(INVALID.into()),
                };
                for byte in &mut plain[start..at] {
                    if !matches!(byte, b'\r' | b'\n') {
                        *byte = b' ';
                    }
                }
                continue;
            }
            b'"' => loop {
                match source.get(at) {
                    Some(b'\\') => at += 2,
                    Some(b'"') => {
                        at += 1;
                        break;
                    }
                    Some(_) => at += 1,
                    None => return Err(INVALID.into()),
                }
            },
            b'}' | b']' => {
                if let Some(previous) = tokens.last().filter(|token| token.kind == b',') {
                    if tokens.len() < 2
                        || matches!(tokens[tokens.len() - 2].kind, b'{' | b'[' | b',')
                    {
                        return Err(INVALID.into());
                    }
                    plain[previous.range.start] = b' ';
                }
            }
            b'{' | b'[' | b':' | b',' => {}
            _ => {
                while source.get(at).is_some_and(|b| {
                    !matches!(b, b' ' | b'\r' | b'\n' | b'\t' | b'}' | b']' | b',' | b'/')
                }) {
                    at += 1;
                }
            }
        }
        tokens.push(Token {
            kind,
            range: start..at,
        });
    }
    Ok((tokens, plain))
}

struct Member {
    key: usize,
    value: Range<usize>,
    comma: Option<usize>,
}

fn members(tokens: &[Token], object: usize) -> (Vec<Member>, usize) {
    let mut at = object + 1;
    let mut fields = Vec::new();
    while tokens[at].kind != b'}' {
        let key = at;
        let start = key + 2;
        at = start + 1;
        if matches!(tokens[start].kind, b'{' | b'[') {
            let mut depth = 1;
            while depth > 0 {
                match tokens[at].kind {
                    b'{' | b'[' => depth += 1,
                    b'}' | b']' => depth -= 1,
                    _ => {}
                }
                at += 1;
            }
        }
        let comma = (tokens[at].kind == b',').then_some(at);
        fields.push(Member {
            key,
            value: start..at,
            comma,
        });
        at += usize::from(comma.is_some());
    }
    (fields, at)
}

impl Document {
    pub fn decode(&self) -> Result<Map<String, Value>, String> {
        let (_, plain) = tokens(&self.0)?;
        serde_json::from_slice(&plain).map_err(|_| INVALID.into())
    }

    pub fn into_bytes(self) -> Vec<u8> {
        self.0.into_bytes()
    }

    pub fn set(
        &mut self,
        sections: &[&str],
        key: &str,
        value: Option<&Value>,
    ) -> Result<(), String> {
        let (tokens, plain) = tokens(&self.0)?;
        // Validate the grammar before using token indexes.
        serde_json::from_slice::<Map<String, Value>>(&plain).map_err(|_| INVALID)?;
        let mut object = 0;
        let path: Vec<_> = sections.iter().copied().chain([key]).collect();
        for (depth, name) in path.iter().enumerate() {
            if tokens[object].kind != b'{' {
                return Err(INVALID.into());
            }
            let (fields, close) = members(&tokens, object);
            let mut matches = fields.iter().enumerate().filter(|(_, field)| {
                serde_json::from_str::<String>(&self.0[tokens[field.key].range.clone()])
                    .is_ok_and(|key| key == *name)
            });
            let field = matches.next();
            if matches.next().is_some() {
                return Err("配置包含同名字段，请先在客户端中修复".into());
            }
            if let Some((index, field)) = field {
                if depth < sections.len() {
                    object = field.value.start;
                    continue;
                }
                let range =
                    tokens[field.value.start].range.start..tokens[field.value.end - 1].range.end;
                if let Some(value) = value {
                    self.0.replace_range(range, &value.to_string());
                } else {
                    let comma = field
                        .comma
                        .or_else(|| index.checked_sub(1).and_then(|i| fields[i].comma));
                    let key = tokens[field.key].range.clone();
                    let colon = tokens[field.key + 1].range.clone();
                    let mut remove = vec![key, colon, range];
                    if let Some(comma) = comma {
                        remove.push(tokens[comma].range.clone());
                    }
                    remove.sort_unstable_by_key(|range| range.start);
                    for range in remove.into_iter().rev() {
                        self.0.replace_range(range, "");
                    }
                }
            } else if let Some(value) = value {
                let entry = path[depth + 1..]
                    .iter()
                    .rev()
                    .fold(value.clone(), |value, &name| {
                        Value::Object(Map::from_iter([(name.to_owned(), value)]))
                    });
                let key = Value::String((*name).to_owned());
                let last = fields.last();
                let trailing = last.is_some_and(|field| field.comma.is_some());
                let close = tokens[close].range.start;
                let line = self.0[..close].rfind('\n').map_or(0, |offset| offset + 1);
                let indent: String = self.0[line..close]
                    .chars()
                    .take_while(|c| matches!(c, ' ' | '\t'))
                    .collect();
                let newline = if self.0.contains("\r\n") {
                    "\r\n"
                } else {
                    "\n"
                };
                let comma = if trailing { "," } else { "" };
                self.0.insert_str(
                    close,
                    &format!("{newline}{indent}  {key}: {entry}{comma}{newline}{indent}"),
                );
                if let Some(field) = last.filter(|field| field.comma.is_none()) {
                    self.0.insert(tokens[field.value.end - 1].range.end, ',');
                }
            }
            return Ok(());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn patches_nested_jsonc_without_removing_comments_or_touching_siblings() {
        let mut doc = document(Some(b"{\r\n  // comment\r\n  \"mcp\": {\"servers\": {\"a\": {\"url\":\"https://a/*\"}, /* b */ \"b\": {},}},\r\n  \"theme\": \"dark\", // trailing\r\n}\r\n")).unwrap();
        doc.set(&["mcp", "servers"], "a", None).unwrap();
        doc.set(&["mcp", "servers", "b"], "disabled", Some(&json!(true)))
            .unwrap();
        doc.set(&["mcp", "servers"], "c", Some(&json!({"type":"local"})))
            .unwrap();
        assert_eq!(
            doc.decode().unwrap()["mcp"]["servers"],
            json!({"b":{"disabled":true},"c":{"type":"local"}})
        );
        assert!(doc.0.contains("/* b */"));
        assert!(doc.0.contains("\"theme\": \"dark\", // trailing"));
        assert!(doc.0.contains("  // comment\r\n"));
    }

    #[test]
    fn rejects_duplicates_at_the_edit_location_and_invalid_trailing_commas() {
        let mut doc = document(Some(br#"{"model":1,"model":2}"#)).unwrap();
        assert!(doc.set(&[], "model", Some(&json!(3))).is_err());
        assert!(document(Some(br#"{",": [,]}"#)).unwrap().decode().is_err());
    }
}
