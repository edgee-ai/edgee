//! Throughput micro-benchmarks for the hot compressor entry points.
//!
//! Run with:
//!   cargo bench -p edgee-compressor
//!
//! Each bench measures one realistic tool output through the public
//! [`compress_tool_output`] entry point (or its Codex equivalent), so a
//! regression in the per-tool strategy, the segment-protection helper,
//! or the marker pipeline shows up as a slowdown here.

use criterion::{Criterion, black_box, criterion_group, criterion_main};

use edgee_compressor::{
    claude_compressor_for, compress_claude_tool_with_segment_protection,
    compress_codex_tool_output, compress_tool_output,
};

fn bench_bash_git_diff(c: &mut Criterion) {
    let args = r#"{"command":"git diff HEAD~50"}"#;
    // Build a synthetic diff body with a realistic number of hunks.
    let mut output = String::new();
    output.push_str("diff --git a/src/main.rs b/src/main.rs\n");
    output.push_str("index 1234..5678 100644\n");
    output.push_str("--- a/src/main.rs\n");
    output.push_str("+++ b/src/main.rs\n");
    for hunk in 0..40 {
        output.push_str(&format!("@@ -{0},20 +{0},22 @@\n", hunk * 25 + 1));
        for ln in 0..20 {
            output.push_str(&format!(" context line {ln} of hunk {hunk}\n"));
        }
        output.push_str("-old line\n+new line\n");
    }

    c.bench_function("bash_git_diff_40_hunks", |b| {
        b.iter(|| {
            let _ = compress_tool_output(black_box("Bash"), black_box(args), black_box(&output));
        });
    });
}

fn bench_read_rust(c: &mut Criterion) {
    let args = r#"{"file_path":"/repo/src/lib.rs"}"#;
    // 800-line Rust file with 50 % comment lines so the filter has work to do.
    let mut lines = Vec::new();
    for i in 1..=800 {
        if i % 2 == 0 {
            lines.push(format!("     {i}\t// commentary about line {i}"));
        } else {
            lines.push(format!(
                "     {i}\tlet value_{i} = compute(arg_a, arg_b, arg_c);"
            ));
        }
    }
    let output = lines.join("\n");

    c.bench_function("read_rust_800_lines", |b| {
        b.iter(|| {
            let _ = compress_tool_output(black_box("Read"), black_box(args), black_box(&output));
        });
    });
}

fn bench_grep_content(c: &mut Criterion) {
    let args = r#"{"output_mode":"content","pattern":"TODO"}"#;
    let mut output = String::new();
    for f in 0..50 {
        for ln in 0..40 {
            output.push_str(&format!(
                "src/module_{f}/file_{f}.rs:{}:    TODO: refactor caller\n",
                ln * 7 + 1
            ));
        }
    }

    c.bench_function("grep_content_2000_matches", |b| {
        b.iter(|| {
            let _ = compress_tool_output(black_box("Grep"), black_box(args), black_box(&output));
        });
    });
}

fn bench_glob_paths(c: &mut Criterion) {
    let args = r#"{"pattern":"**/*.rs"}"#;
    let mut output = String::new();
    let dirs = ["src/alpha", "src/beta", "src/gamma", "src/delta", "tests"];
    for i in 0..400 {
        output.push_str(&format!("{}/file_{i}.rs\n", dirs[i % dirs.len()]));
    }

    c.bench_function("glob_400_paths", |b| {
        b.iter(|| {
            let _ = compress_tool_output(black_box("Glob"), black_box(args), black_box(&output));
        });
    });
}

fn bench_segment_protection_no_reminder(c: &mut Criterion) {
    // Hot fast-path: no `<system-reminder>` tag → early-out skips the regex.
    let compressor = claude_compressor_for("Glob").unwrap();
    let args = r#"{"pattern":"**/*.rs"}"#;
    let mut output = String::new();
    let dirs = ["src/alpha", "src/beta", "src/gamma"];
    for i in 0..200 {
        output.push_str(&format!("{}/file_{i}.rs\n", dirs[i % dirs.len()]));
    }

    c.bench_function("segment_protection_no_reminder", |b| {
        b.iter(|| {
            let _ = compress_claude_tool_with_segment_protection(
                compressor,
                black_box(args),
                black_box(&output),
            );
        });
    });
}

fn bench_codex_strip_and_compress(c: &mut Criterion) {
    let args = r#"{"command":"ls -la"}"#;
    let mut body = String::from("total 9999\n");
    for i in 0..200 {
        body.push_str(&format!(
            "-rw-r--r-- 1 user staff {i:>5} Jan 1 12:00 file_{i}.txt\n"
        ));
    }
    let output = format!("Exit code: 0\nWall time: 0 seconds\nOutput:\n{body}");

    c.bench_function("codex_shell_command_200_files", |b| {
        b.iter(|| {
            let _ = compress_codex_tool_output(
                black_box("shell_command"),
                black_box(args),
                black_box(&output),
            );
        });
    });
}

criterion_group!(
    benches,
    bench_bash_git_diff,
    bench_read_rust,
    bench_grep_content,
    bench_glob_paths,
    bench_segment_protection_no_reminder,
    bench_codex_strip_and_compress,
);
criterion_main!(benches);
