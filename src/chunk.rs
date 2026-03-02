use anyhow::Result;
use tracing::warn;

const MAX_CHUNK_CHARS: usize = 48_000;

#[derive(Debug, Clone)]
pub struct Chunk {
    pub file_path: String,
    pub start_line: usize,
    pub end_line: usize,
    pub item_name: String,
    pub text: String,
    pub language: String,
}

pub fn chunk_file(file_path: &str, source: &str) -> Result<Vec<Chunk>> {
    let chunks = if file_path.ends_with(".rs") {
        chunk_rust(file_path, source)?
    } else if file_path.ends_with(".md") {
        chunk_markdown(file_path, source)
    } else {
        vec![Chunk {
            file_path: file_path.to_string(),
            start_line: 1,
            end_line: source.lines().count().max(1),
            item_name: file_path.to_string(),
            text: source.to_string(),
            language: "text".to_string(),
        }]
    };

    Ok(chunks
        .into_iter()
        .map(|mut c| {
            if c.text.len() > MAX_CHUNK_CHARS {
                warn!(
                    file = c.file_path,
                    item = c.item_name,
                    len = c.text.len(),
                    "chunk exceeds {}chars, truncating",
                    MAX_CHUNK_CHARS
                );
                c.text.truncate(MAX_CHUNK_CHARS);
            }
            c
        })
        .filter(|c| !c.text.trim().is_empty())
        .collect())
}

// --- Rust chunking via tree-sitter ---

const TOP_LEVEL_KINDS: &[&str] = &[
    "function_item",
    "struct_item",
    "enum_item",
    "impl_item",
    "trait_item",
    "mod_item",
    "const_item",
    "static_item",
    "type_item",
    "macro_definition",
];

fn chunk_rust(file_path: &str, source: &str) -> Result<Vec<Chunk>> {
    let mut parser = tree_sitter::Parser::new();
    let language = tree_sitter_rust::LANGUAGE;
    parser
        .set_language(&language.into())
        .map_err(|e| anyhow::anyhow!("failed to set tree-sitter language: {}", e))?;

    let tree = parser
        .parse(source, None)
        .ok_or_else(|| anyhow::anyhow!("tree-sitter failed to parse {}", file_path))?;

    let root = tree.root_node();
    let source_bytes = source.as_bytes();
    let lines: Vec<&str> = source.lines().collect();

    let mut chunks = Vec::new();
    let mut covered_ranges: Vec<(usize, usize)> = Vec::new(); // (start_byte, end_byte)

    let mut cursor = root.walk();
    for child in root.children(&mut cursor) {
        let kind = child.kind();
        if !TOP_LEVEL_KINDS.contains(&kind) {
            continue;
        }

        let start_line = child.start_position().row + 1; // 1-based
        let end_line = child.end_position().row + 1;
        let line_count = end_line - start_line + 1;

        // For large impl blocks, split into individual methods
        if kind == "impl_item" && line_count > 100 {
            let impl_name = extract_impl_name(&child, source_bytes);
            let mut impl_chunks =
                split_impl_methods(file_path, &child, source_bytes, &lines, &impl_name);
            for c in &impl_chunks {
                covered_ranges.push((
                    byte_offset_for_line(source, c.start_line - 1),
                    byte_offset_for_line(source, c.end_line),
                ));
            }
            chunks.append(&mut impl_chunks);
        } else {
            let item_name = extract_item_name(&child, source_bytes, kind);
            let text = child
                .utf8_text(source_bytes)
                .unwrap_or_default()
                .to_string();

            covered_ranges.push((child.start_byte(), child.end_byte()));

            chunks.push(Chunk {
                file_path: file_path.to_string(),
                start_line,
                end_line,
                item_name,
                text,
                language: "rust".to_string(),
            });
        }
    }

    // Collect preamble (lines not covered by any declaration)
    let preamble = collect_preamble(file_path, source, &covered_ranges);
    if let Some(p) = preamble {
        chunks.insert(0, p);
    }

    Ok(chunks)
}

fn extract_item_name(node: &tree_sitter::Node, source: &[u8], kind: &str) -> String {
    // Try to find the name child node
    if let Some(name_node) = node.child_by_field_name("name") {
        return name_node.utf8_text(source).unwrap_or_default().to_string();
    }

    // For macro_definition, look for the identifier after "macro_rules!"
    if kind == "macro_definition" {
        let mut cursor = node.walk();
        for child in node.children(&mut cursor) {
            if child.kind() == "identifier" || child.kind() == "!" {
                continue;
            }
            if child.kind() == "identifier" {
                return child.utf8_text(source).unwrap_or_default().to_string();
            }
        }
    }

    // Fallback: first line trimmed
    let text = node.utf8_text(source).unwrap_or_default();
    text.lines()
        .next()
        .unwrap_or("unknown")
        .trim()
        .chars()
        .take(60)
        .collect()
}

fn extract_impl_name(node: &tree_sitter::Node, source: &[u8]) -> String {
    // impl <Type> or impl <Trait> for <Type>
    if let Some(type_node) = node.child_by_field_name("type") {
        let type_name = type_node.utf8_text(source).unwrap_or_default().to_string();
        if let Some(trait_node) = node.child_by_field_name("trait") {
            let trait_name = trait_node.utf8_text(source).unwrap_or_default().to_string();
            return format!("{}<{}>", type_name, trait_name);
        }
        return type_name;
    }
    "impl".to_string()
}

fn split_impl_methods(
    file_path: &str,
    impl_node: &tree_sitter::Node,
    source: &[u8],
    _lines: &[&str],
    impl_name: &str,
) -> Vec<Chunk> {
    let mut chunks = Vec::new();

    // Find the body (declaration_list)
    let body = impl_node
        .children(&mut impl_node.walk())
        .find(|c| c.kind() == "declaration_list");

    let Some(body) = body else {
        // No body, emit the whole impl as one chunk
        let text = impl_node.utf8_text(source).unwrap_or_default().to_string();
        chunks.push(Chunk {
            file_path: file_path.to_string(),
            start_line: impl_node.start_position().row + 1,
            end_line: impl_node.end_position().row + 1,
            item_name: impl_name.to_string(),
            text,
            language: "rust".to_string(),
        });
        return chunks;
    };

    let mut cursor = body.walk();
    for child in body.children(&mut cursor) {
        let kind = child.kind();
        if kind == "function_item" || kind == "const_item" || kind == "type_item" {
            let method_name = child
                .child_by_field_name("name")
                .and_then(|n| n.utf8_text(source).ok())
                .unwrap_or("unknown");
            let item_name = format!("{}::{}", impl_name, method_name);
            let text = child.utf8_text(source).unwrap_or_default().to_string();
            let start_line = child.start_position().row + 1;
            let end_line = child.end_position().row + 1;

            chunks.push(Chunk {
                file_path: file_path.to_string(),
                start_line,
                end_line,
                item_name,
                text,
                language: "rust".to_string(),
            });
        }
    }

    // If no methods found (unlikely), fall back to the whole impl
    if chunks.is_empty() {
        let text = impl_node.utf8_text(source).unwrap_or_default().to_string();
        chunks.push(Chunk {
            file_path: file_path.to_string(),
            start_line: impl_node.start_position().row + 1,
            end_line: impl_node.end_position().row + 1,
            item_name: impl_name.to_string(),
            text,
            language: "rust".to_string(),
        });
    }

    chunks
}

fn byte_offset_for_line(source: &str, line_idx: usize) -> usize {
    source
        .lines()
        .take(line_idx)
        .map(|l| l.len() + 1) // +1 for newline
        .sum()
}

fn collect_preamble(file_path: &str, source: &str, covered: &[(usize, usize)]) -> Option<Chunk> {
    // Collect bytes not covered by any declaration
    let mut preamble_lines = Vec::new();
    let mut first_line = None;
    let mut last_line = None;

    for (line_idx, line) in source.lines().enumerate() {
        let line_start = byte_offset_for_line(source, line_idx);
        let line_end = line_start + line.len();

        let is_covered = covered
            .iter()
            .any(|(s, e)| line_start >= *s && line_end <= *e);

        if !is_covered && !line.trim().is_empty() {
            if first_line.is_none() {
                first_line = Some(line_idx + 1);
            }
            last_line = Some(line_idx + 1);
            preamble_lines.push(line);
        }
    }

    if preamble_lines.is_empty() {
        return None;
    }

    let text = preamble_lines.join("\n");
    Some(Chunk {
        file_path: file_path.to_string(),
        start_line: first_line.unwrap_or(1),
        end_line: last_line.unwrap_or(1),
        item_name: "preamble".to_string(),
        text,
        language: "rust".to_string(),
    })
}

// --- Markdown chunking ---

fn chunk_markdown(file_path: &str, source: &str) -> Vec<Chunk> {
    let lines: Vec<&str> = source.lines().collect();
    if lines.is_empty() {
        return vec![];
    }

    let mut chunks = Vec::new();
    let mut current_heading = String::new();
    let mut current_start = 0usize; // 0-indexed line
    let mut current_lines: Vec<&str> = Vec::new();

    for (idx, line) in lines.iter().enumerate() {
        if is_heading(line) {
            // Flush previous section
            if !current_lines.is_empty() {
                let text = current_lines.join("\n");
                chunks.push(Chunk {
                    file_path: file_path.to_string(),
                    start_line: current_start + 1,
                    end_line: idx, // previous line (1-based = idx)
                    item_name: if current_heading.is_empty() {
                        file_path.to_string()
                    } else {
                        current_heading.clone()
                    },
                    text,
                    language: "markdown".to_string(),
                });
            }
            current_heading = line.trim_start_matches('#').trim().to_string();
            current_start = idx;
            current_lines = vec![line];
        } else {
            current_lines.push(line);
        }
    }

    // Flush last section
    if !current_lines.is_empty() {
        let text = current_lines.join("\n");
        chunks.push(Chunk {
            file_path: file_path.to_string(),
            start_line: current_start + 1,
            end_line: lines.len(),
            item_name: if current_heading.is_empty() {
                file_path.to_string()
            } else {
                current_heading
            },
            text,
            language: "markdown".to_string(),
        });
    }

    chunks
}

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    // Match #{1,3} followed by space
    if let Some(rest) = trimmed.strip_prefix('#') {
        if rest.starts_with(' ') {
            return true;
        }
        if let Some(rest2) = rest.strip_prefix('#') {
            if rest2.starts_with(' ') {
                return true;
            }
            if let Some(rest3) = rest2.strip_prefix('#') {
                if rest3.starts_with(' ') {
                    return true;
                }
            }
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn should_chunk_simple_rust_file() {
        let source = r#"use std::io;

fn hello() {
    println!("hello");
}

struct Foo {
    bar: i32,
}
"#;
        let chunks = chunk_file("test.rs", source).unwrap();
        assert!(chunks.len() >= 2); // hello fn + Foo struct (+ maybe preamble)

        let fn_chunk = chunks.iter().find(|c| c.item_name == "hello").unwrap();
        assert_eq!(fn_chunk.language, "rust");
        assert!(fn_chunk.text.contains("fn hello()"));

        let struct_chunk = chunks.iter().find(|c| c.item_name == "Foo").unwrap();
        assert!(struct_chunk.text.contains("struct Foo"));
    }

    #[test]
    fn should_chunk_markdown_by_headings() {
        let source =
            "# Title\nSome intro text.\n## Section A\nContent A.\n## Section B\nContent B.\n";
        let chunks = chunk_file("readme.md", source).unwrap();
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].item_name, "Title");
        assert_eq!(chunks[1].item_name, "Section A");
        assert_eq!(chunks[2].item_name, "Section B");
    }

    #[test]
    fn should_handle_markdown_without_headings() {
        let source = "Just some text\nwithout headings.\n";
        let chunks = chunk_file("notes.md", source).unwrap();
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].item_name, "notes.md");
    }
}
