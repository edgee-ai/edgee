#!/usr/bin/env bash
# Destructive fixtures are restricted to a disposable Linux GitHub Actions job.
# Never run against a developer's machine or an existing administrator policy.
set -euo pipefail

if [[ "${GITHUB_ACTIONS:-}" != true || "${RUNNER_OS:-}" != Linux || "${EDGEE_ADMIN_FIXTURE:-}" != 1 ]]; then
  echo 'This test requires an explicitly enabled disposable Linux GitHub Actions runner.' >&2
  exit 1
fi
if [[ -e /etc/edgee || -L /etc/edgee || -e /usr/lib/edgee-sandbox-ci || -L /usr/lib/edgee-sandbox-ci ]]; then
  echo 'Refusing to replace existing /etc/edgee or /usr/lib/edgee-sandbox-ci.' >&2
  exit 1
fi

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
edgee_binary="$(realpath "$repo_root/target/debug/edgee")"
node_binary="$(realpath "$(command -v node)")"
fixture="$(mktemp -d)"
created_policy=0
created_runtime=0
cleanup() {
  if [[ "$created_policy" == 1 ]]; then sudo rm -rf -- /etc/edgee; fi
  if [[ "$created_runtime" == 1 ]]; then sudo rm -rf -- /usr/lib/edgee-sandbox-ci; fi
  rm -rf -- "$fixture"
}
trap cleanup EXIT

sudo mkdir /usr/lib/edgee-sandbox-ci
created_runtime=1
sudo mkdir /etc/edgee
created_policy=1
sudo cp -a "$repo_root/scripts/sandbox-runtime" /usr/lib/edgee-sandbox-ci/runtime
sudo install -m 0755 "$node_binary" /usr/lib/edgee-sandbox-ci/node
sudo chown -hR root:root /usr/lib/edgee-sandbox-ci /etc/edgee
sudo chmod -R go-w /usr/lib/edgee-sandbox-ci /etc/edgee
# Show the trust-chain metadata in CI logs without changing shared parents.
stat -c "%A %U:%G %n" / /etc /etc/edgee /usr /usr/lib /usr/lib/edgee-sandbox-ci
cat > "$fixture/policy.toml" <<'POLICY'
mandatory = true
allowed_domains = ["api.edgee.ai"]
node_binary = "/usr/lib/edgee-sandbox-ci/node"
runtime_root = "/usr/lib/edgee-sandbox-ci/runtime"
POLICY
sudo install -m 0644 "$fixture/policy.toml" /etc/edgee/sandbox.toml

workspace="$fixture/project with spaces"
mkdir -p "$workspace/.edgee"
git -C "$workspace" init --quiet
cd "$workspace"

expect_refusal() {
  local expected="$1"
  shift
  local output status
  set +e
  output="$(EDGEE_NO_UPDATE_CHECK=1 "$edgee_binary" "$@" 2>&1)"
  status=$?
  set -e
  if [[ "$status" == 0 || "$output" != *"$expected"* ]]; then
    printf 'Expected refusal containing %s; exit=%s; output:\n%s\n' "$expected" "$status" "$output" >&2
    exit 1
  fi
}

# None of these launches requests --sandbox. Mandatory policy must be selected
# before unsupported target handling or authentication. The alias-shaped call
# also exercises automatic Git-root discovery from a subdirectory.
mandatory_message='mandatory sandbox policy cannot be bypassed'
expect_refusal "$mandatory_message" launch --workspace "$workspace" opencode
printf 'mandatory = false\nallowed_domains = ["example.com"]\n' > .edgee/sandbox.toml
expect_refusal "$mandatory_message" launch --workspace "$workspace" opencode
export EDGEE_SANDBOX=false EDGEE_SANDBOX_DISABLED=1
expect_refusal "$mandatory_message" launch --workspace "$workspace" opencode --sandbox=false
mkdir nested
cd nested
expect_refusal "$mandatory_message" launch opencode -- --sandbox=false
cd ..
unset EDGEE_SANDBOX EDGEE_SANDBOX_DISABLED
echo 'PASS mandatory policy, repository override rejection, environment override rejection, alias-shaped launch'

sudo chmod 0666 /etc/edgee/sandbox.toml
expect_refusal 'must be root-owned and not group/world writable' launch opencode
sudo chmod 0644 /etc/edgee/sandbox.toml
sudo mv /etc/edgee/sandbox.toml /etc/edgee/real-policy.toml
sudo ln -s real-policy.toml /etc/edgee/sandbox.toml
expect_refusal 'must not be symlinks' launch opencode
sudo rm /etc/edgee/sandbox.toml
sudo mv /etc/edgee/real-policy.toml /etc/edgee/sandbox.toml
echo 'PASS writable-policy and symlink-policy rejection'

runtime_package=/usr/lib/edgee-sandbox-ci/runtime/node_modules/@anthropic-ai/sandbox-runtime
sudo chmod o+w "$runtime_package/dist/index.js"
expect_refusal 'runtime must not be group/world writable' launch opencode
sudo chmod o-w "$runtime_package/dist/index.js"
sudo ln -s "$fixture" /usr/lib/edgee-sandbox-ci/runtime/escape
expect_refusal 'runtime symlink escapes its root' launch opencode
sudo rm /usr/lib/edgee-sandbox-ci/runtime/escape
sudo mv /usr/lib/edgee-sandbox-ci/runtime /usr/lib/edgee-sandbox-ci/runtime-missing
expect_refusal 'Cannot verify ownership of administrator sandbox policy' launch opencode
sudo mv /usr/lib/edgee-sandbox-ci/runtime-missing /usr/lib/edgee-sandbox-ci/runtime
sudo mv "$runtime_package/package.json" "$runtime_package/package-missing.json"
expect_refusal 'Sandbox runtime is missing' launch claude
sudo mv "$runtime_package/package-missing.json" "$runtime_package/package.json"
expect_refusal "$mandatory_message" launch opencode
echo 'PASS writable-runtime, escaping-runtime-symlink, missing-runtime and missing-package rejection'
echo 'PASS administrator policy integration (Linux disposable fixture)'
