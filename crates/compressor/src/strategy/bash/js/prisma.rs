use super::BashCompressor;

pub struct PrismaCompressor;

impl BashCompressor for PrismaCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        if output.trim().is_empty() {
            return None;
        }

        let subcommand = parse_prisma_subcommand(command);
        let filtered = match subcommand {
            "generate" => filter_prisma_generate(output),
            "migrate" => filter_prisma_migrate(output),
            "db" => filter_prisma_db(output),
            _ => return None,
        };

        if filtered == output.trim() {
            return None;
        }

        Some(filtered)
    }
}

fn parse_prisma_subcommand(command: &str) -> &str {
    let mut found_prisma = false;
    for arg in command.split_whitespace() {
        if arg == "prisma" {
            found_prisma = true;
            continue;
        }
        if found_prisma && !arg.starts_with('-') {
            return arg;
        }
    }
    ""
}

fn filter_prisma_generate(output: &str) -> String {
    let mut models = 0;
    let mut enums = 0;
    let mut types = 0;
    let mut output_path = String::new();

    for line in output.lines() {
        if line.contains('█')
            || line.contains('▀')
            || line.contains('▄')
            || line.contains('┌')
            || line.contains('└')
            || line.contains('│')
        {
            continue;
        }

        if line.contains("model")
            && line.contains("generated")
            && let Some(num) = extract_number(line)
        {
            models = num;
        }
        if line.contains("enum")
            && let Some(num) = extract_number(line)
        {
            enums = num;
        }
        if line.contains("type")
            && let Some(num) = extract_number(line)
        {
            types = num;
        }

        if line.contains("node_modules") && line.contains("@prisma") {
            output_path = line.trim().to_string();
        }
    }

    let mut result = String::new();
    result.push_str("Prisma Client generated\n");

    if models > 0 || enums > 0 || types > 0 {
        result.push_str(&format!(
            "  • {} models, {} enums, {} types\n",
            models, enums, types
        ));
    }

    if !output_path.is_empty() {
        result.push_str("  • Output: node_modules/@prisma/client\n");
    }

    result.trim().to_string()
}

fn filter_prisma_migrate(output: &str) -> String {
    let mut tables_added = 0;
    let mut tables_modified = 0;
    let mut relations = Vec::new();
    let mut indexes = Vec::new();
    let mut applied = false;
    let mut migration_name = String::new();

    for line in output.lines() {
        if line.contains("migration")
            && line.contains('_')
            && let Some(pos) = line.find("202")
        {
            let end = line[pos..]
                .find(|c: char| c.is_whitespace())
                .unwrap_or(line.len() - pos);
            migration_name = line[pos..pos + end].to_string();
        }

        if line.contains("CREATE TABLE") {
            tables_added += 1;
        }
        if line.contains("ALTER TABLE") {
            tables_modified += 1;
        }
        if (line.contains("FOREIGN KEY") || line.contains("REFERENCES"))
            && let Some(table) = extract_table_name(line)
        {
            relations.push(table);
        }
        if (line.contains("CREATE INDEX") || line.contains("CREATE UNIQUE INDEX"))
            && let Some(idx) = extract_index_name(line)
        {
            indexes.push(idx);
        }

        if line.contains("applied") || line.contains('✓') {
            applied = true;
        }
    }

    let mut result = String::new();

    if !migration_name.is_empty() {
        result.push_str(&format!("Migration: {}\n", migration_name));
    }

    result.push_str("Changes:\n");
    if tables_added > 0 {
        result.push_str(&format!("  + {} table(s)\n", tables_added));
    }
    if tables_modified > 0 {
        result.push_str(&format!("  ~ {} table(s) modified\n", tables_modified));
    }
    if !relations.is_empty() {
        result.push_str(&format!("  + {} relation(s)\n", relations.len()));
    }
    if !indexes.is_empty() {
        result.push_str(&format!("  ~ {} index(es)\n", indexes.len()));
    }

    if applied {
        result.push_str("\nApplied | Pending: 0\n");
    }

    result.trim().to_string()
}

fn filter_prisma_db(output: &str) -> String {
    let mut tables_added = 0;
    let mut columns_modified = 0;
    let mut dropped = 0;

    for line in output.lines() {
        if line.contains("CREATE TABLE") {
            tables_added += 1;
        }
        if line.contains("ALTER") || line.contains("ADD COLUMN") {
            columns_modified += 1;
        }
        if line.contains("DROP") {
            dropped += 1;
        }
    }

    let mut result = String::new();
    result.push_str("Schema pushed to database\n");

    if tables_added > 0 || columns_modified > 0 || dropped > 0 {
        result.push_str(&format!(
            "  + {} tables, ~ {} columns, - {} dropped\n",
            tables_added, columns_modified, dropped
        ));
    }

    result.trim().to_string()
}

fn extract_number(line: &str) -> Option<usize> {
    line.split_whitespace()
        .find_map(|word| word.parse::<usize>().ok())
}

fn extract_table_name(line: &str) -> Option<String> {
    if line.contains("TABLE") {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "TABLE" && i + 1 < parts.len() {
                return Some(
                    parts[i + 1]
                        .trim_matches(|c| c == '`' || c == '"' || c == ';')
                        .to_string(),
                );
            }
        }
    }
    None
}

fn extract_index_name(line: &str) -> Option<String> {
    if line.contains("INDEX") {
        let parts: Vec<&str> = line.split_whitespace().collect();
        for (i, part) in parts.iter().enumerate() {
            if *part == "INDEX" && i + 1 < parts.len() {
                return Some(
                    parts[i + 1]
                        .trim_matches(|c| c == '`' || c == '"' || c == ';')
                        .to_string(),
                );
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_generate() {
        let output = r#"
Prisma schema loaded from prisma/schema.prisma

✔ Generated Prisma Client (v5.7.0) to ./node_modules/@prisma/client in 234ms

Start by importing your Prisma Client:

import { PrismaClient } from '@prisma/client'

42 models, 18 enums, 890 types generated
"#;
        let result = filter_prisma_generate(output);
        assert!(result.contains("Prisma Client generated"));
        assert!(!result.contains("Prisma schema loaded"));
        assert!(!result.contains("Start by importing"));
    }

    #[test]
    fn test_filter_migrate_dev() {
        let output = r#"
Applying migration 20260128_add_sessions

CREATE TABLE "Session" (
  "id" TEXT NOT NULL,
  "userId" TEXT NOT NULL,
  FOREIGN KEY ("userId") REFERENCES "User"("id")
);

CREATE INDEX "session_status_idx" ON "Session"("status");

✓ Migration applied
"#;
        let result = filter_prisma_migrate(output);
        assert!(result.contains("20260128_add_sessions"));
        assert!(result.contains("+ 1 table"));
        assert!(result.contains("Applied"));
    }

    #[test]
    fn test_filter_db_push() {
        let output = r#"
CREATE TABLE "User" (
  "id" TEXT NOT NULL,
  "email" TEXT NOT NULL
);

ALTER TABLE "User" ADD COLUMN "name" TEXT;
"#;
        let result = filter_prisma_db(output);
        assert!(result.contains("Schema pushed to database"));
        assert!(result.contains("+ 1 tables"));
    }

    #[test]
    fn test_extract_number() {
        assert_eq!(extract_number("42 models generated"), Some(42));
        assert_eq!(extract_number("no numbers here"), None);
    }

    #[test]
    fn test_compressor_empty() {
        let c = PrismaCompressor;
        assert!(c.compress("npx prisma generate", "").is_none());
    }

    #[test]
    fn test_compressor_reduces_length() {
        let c = PrismaCompressor;
        let output = r#"
Prisma schema loaded from prisma/schema.prisma

✔ Generated Prisma Client (v5.7.0) to ./node_modules/@prisma/client in 234ms

Start by importing your Prisma Client:

import { PrismaClient } from '@prisma/client'

42 models, 18 enums, 890 types generated
"#;
        let result = c.compress("npx prisma generate", output).unwrap();
        assert!(result.len() < output.len());
    }

    #[test]
    fn test_parse_subcommand() {
        assert_eq!(parse_prisma_subcommand("npx prisma generate"), "generate");
        assert_eq!(parse_prisma_subcommand("npx prisma migrate dev"), "migrate");
        assert_eq!(parse_prisma_subcommand("npx prisma db push"), "db");
    }
}
