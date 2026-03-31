//! Compressor for `docker` command output.
//!
//! Compacts `docker ps` and `docker images` tabular output into
//! a dense, token-efficient format.

use super::BashCompressor;

pub struct DockerCompressor;

impl BashCompressor for DockerCompressor {
    fn compress(&self, command: &str, output: &str) -> Option<String> {
        let subcommand = parse_docker_subcommand(command);
        match subcommand {
            "ps" => Some(compact_docker_ps(output)),
            "images" => Some(compact_docker_images(output)),
            _ => None,
        }
    }
}

fn parse_docker_subcommand(command: &str) -> &str {
    for arg in command.split_whitespace().skip(1) {
        if arg.starts_with('-') {
            continue;
        }
        return arg;
    }
    ""
}

/// Compact `docker ps` tabular output.
///
/// Input is the default table format with headers:
/// CONTAINER ID   IMAGE   COMMAND   CREATED   STATUS   PORTS   NAMES
fn compact_docker_ps(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        return "0 containers\n".to_string();
    }

    // Find column positions from the header line
    let header = lines[0];
    let data_lines: Vec<&str> = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();

    if data_lines.is_empty() {
        return "0 containers\n".to_string();
    }

    let col_positions = parse_header_columns(header);
    let mut out = format!("{} containers:\n", data_lines.len());

    for line in data_lines.iter().take(20) {
        let cols = extract_columns(line, &col_positions);
        let id = cols
            .get("CONTAINER ID")
            .map(|s| if s.len() > 12 { &s[..12] } else { s.as_str() })
            .unwrap_or("");
        let name = cols.get("NAMES").unwrap_or(&String::new()).clone();
        let image = cols.get("IMAGE").unwrap_or(&String::new()).clone();
        let status = cols.get("STATUS").unwrap_or(&String::new()).clone();
        let ports = cols.get("PORTS").unwrap_or(&String::new()).clone();

        let short_image = image.split('/').next_back().unwrap_or(&image);
        let compact_ports = compact_port_string(&ports);

        if compact_ports.is_empty() || compact_ports == "-" {
            out.push_str(&format!("  {} {} ({}) {}\n", id, name, short_image, status));
        } else {
            out.push_str(&format!(
                "  {} {} ({}) {} [{}]\n",
                id, name, short_image, status, compact_ports
            ));
        }
    }

    if data_lines.len() > 20 {
        out.push_str(&format!("  ... +{} more\n", data_lines.len() - 20));
    }

    out
}

/// Compact `docker images` tabular output.
///
/// Input headers: REPOSITORY   TAG   IMAGE ID   CREATED   SIZE
fn compact_docker_images(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    if lines.is_empty() {
        return "0 images\n".to_string();
    }

    let header = lines[0];
    let data_lines: Vec<&str> = lines[1..]
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .collect();

    if data_lines.is_empty() {
        return "0 images\n".to_string();
    }

    let col_positions = parse_header_columns(header);
    let mut out = format!("{} images:\n", data_lines.len());

    for line in data_lines.iter().take(20) {
        let cols = extract_columns(line, &col_positions);
        let repo = cols.get("REPOSITORY").unwrap_or(&String::new()).clone();
        let tag = cols.get("TAG").unwrap_or(&String::new()).clone();
        let size = cols.get("SIZE").unwrap_or(&String::new()).clone();

        let image_name = if tag == "<none>" || tag.is_empty() {
            repo.clone()
        } else {
            format!("{}:{}", repo, tag)
        };

        let short = if image_name.len() > 45 {
            format!("...{}", &image_name[image_name.len() - 42..])
        } else {
            image_name
        };

        out.push_str(&format!("  {} [{}]\n", short, size));
    }

    if data_lines.len() > 20 {
        out.push_str(&format!("  ... +{} more\n", data_lines.len() - 20));
    }

    out
}

/// Parse column header positions from a docker table header line.
/// Returns vec of (column_name, start_position).
fn parse_header_columns(header: &str) -> Vec<(String, usize)> {
    let mut cols = Vec::new();
    let mut i = 0;
    let chars: Vec<char> = header.chars().collect();

    while i < chars.len() {
        // Skip whitespace
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        let start = i;
        // Read column name (may contain spaces like "CONTAINER ID" or "IMAGE ID")
        while i < chars.len()
            && !(i > start
                && chars[i].is_whitespace()
                && i + 1 < chars.len()
                && chars[i + 1].is_whitespace())
        {
            // Check for double-space which separates columns
            i += 1;
        }

        let name = header[start..i].trim().to_string();
        if !name.is_empty() {
            cols.push((name, start));
        }
        // Skip to next non-space
        while i < chars.len() && chars[i].is_whitespace() {
            i += 1;
        }
    }

    cols
}

/// Extract column values from a data line using header positions.
fn extract_columns(
    line: &str,
    col_positions: &[(String, usize)],
) -> std::collections::HashMap<String, String> {
    let mut map = std::collections::HashMap::new();

    for (idx, (name, start)) in col_positions.iter().enumerate() {
        let end = if idx + 1 < col_positions.len() {
            col_positions[idx + 1].1
        } else {
            line.len()
        };

        let start = (*start).min(line.len());
        let end = end.min(line.len());

        if start <= end {
            let value = line.get(start..end).unwrap_or("").trim().to_string();
            map.insert(name.clone(), value);
        }
    }

    map
}

fn compact_port_string(ports: &str) -> String {
    if ports.is_empty() {
        return "-".to_string();
    }

    let port_nums: Vec<&str> = ports
        .split(',')
        .filter_map(|p| p.split("->").next().and_then(|s| s.split(':').next_back()))
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    if port_nums.is_empty() {
        return "-".to_string();
    }

    if port_nums.len() <= 3 {
        port_nums.join(", ")
    } else {
        format!(
            "{}, ... +{}",
            port_nums[..2].join(", "),
            port_nums.len() - 2
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_docker_ps_basic() {
        let input = "CONTAINER ID   IMAGE          COMMAND       CREATED        STATUS        PORTS     NAMES\nabc123def456   nginx:latest   \"nginx -g…\"   2 hours ago    Up 2 hours    80/tcp    web\n";
        let compressor = DockerCompressor;
        let result = compressor.compress("docker ps", input).unwrap();
        assert!(result.contains("1 containers:"));
        assert!(result.contains("web"));
        assert!(result.contains("nginx"));
    }

    #[test]
    fn test_docker_ps_empty() {
        let input = "CONTAINER ID   IMAGE   COMMAND   CREATED   STATUS   PORTS   NAMES\n";
        let compressor = DockerCompressor;
        let result = compressor.compress("docker ps", input).unwrap();
        assert!(result.contains("0 containers"));
    }

    #[test]
    fn test_docker_images_basic() {
        let input = "REPOSITORY   TAG       IMAGE ID       CREATED        SIZE\nnginx        latest    abc123def456   2 weeks ago    187MB\nredis        7.0       def456abc789   3 weeks ago    130MB\n";
        let compressor = DockerCompressor;
        let result = compressor.compress("docker images", input).unwrap();
        assert!(result.contains("2 images:"));
        assert!(result.contains("nginx:latest"));
        assert!(result.contains("redis:7.0"));
    }

    #[test]
    fn test_docker_images_empty() {
        let input = "REPOSITORY   TAG   IMAGE ID   CREATED   SIZE\n";
        let compressor = DockerCompressor;
        let result = compressor.compress("docker images", input).unwrap();
        assert!(result.contains("0 images"));
    }

    #[test]
    fn test_unknown_subcommand() {
        let compressor = DockerCompressor;
        assert!(
            compressor
                .compress("docker exec -it foo bash", "root@abc:/# ")
                .is_none()
        );
    }

    #[test]
    fn test_compact_ports() {
        assert_eq!(compact_port_string(""), "-");
        assert_eq!(compact_port_string("80/tcp"), "80/tcp");
        assert_eq!(compact_port_string("0.0.0.0:8080->80/tcp"), "8080");
    }
}
