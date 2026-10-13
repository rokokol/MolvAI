# The single source of truth for the gate: CI calls these same recipes
# Test runs go through scripts/t.sh (the tests skill): the status is the own status of cargo,
# and the log is read even at exit 0

set shell := ["bash", "-euo", "pipefail", "-c"]

build_cmd := "cargo build --workspace --all-targets"
test_cmd := "cargo test --workspace --no-fail-fast"

default: check

# The full gate: formatting, linter, dependencies, tests, SPDX headers, no stubs
check: fmt clippy lint-deps test spdx-check no-stubs

fmt:
    cargo fmt --all --check

# The linter rules are in [workspace.lints] of the root Cargo.toml, the limits are in clippy.toml
clippy:
    cargo clippy --workspace --all-targets -- -D warnings

# A dependency that nobody uses slows the build and the audit for nothing
lint-deps:
    cargo machete

# Documentation without warnings: a broken link in docs is a deny (see [workspace.lints.rustdoc])
doc:
    cargo doc --no-deps --workspace

build:
    {{build_cmd}}

# Tests with an honest verdict and a log that is read
test:
    scripts/t.sh run -- {{test_cmd}}

# N runs in a row: do the results differ on the same code
flaky N:
    scripts/t.sh flaky {{N}} -- {{test_cmd}}

# Break the guarantees from tests/defects.sh one by one and require the suite to notice
falsify *FILTER:
    scripts/t.sh falsify -d tests/defects.sh -b '{{build_cmd}}' {{FILTER}} -- {{test_cmd}}

# Which commit broke the suite, starting from GOOD
bisect GOOD:
    scripts/t.sh bisect {{GOOD}} -b '{{build_cmd}}' -- {{test_cmd}}

# Every source file has an SPDX header
spdx-check:
    #!/usr/bin/env bash
    set -euo pipefail
    missing=$(grep -rL --include='*.rs' --include='*.ts' --include='*.tsx' \
        --exclude-dir=target --exclude-dir=node_modules --exclude-dir=dist --exclude-dir=gen \
        'SPDX-License-Identifier: MIT' crates || true)
    if [ -n "$missing" ]; then
        echo "No SPDX header:"; echo "$missing"; exit 1
    fi
    echo "SPDX: ok"

# The product code has no todo!/unimplemented! stubs
no-stubs:
    #!/usr/bin/env bash
    set -euo pipefail
    if grep -rnE 'todo!\(|unimplemented!\(' --include='*.rs' --exclude-dir=target crates | grep -v '/tests/'; then
        echo "Stubs in the product code"; exit 1
    fi
    echo "No stubs"

deny:
    cargo deny check

audit:
    cargo audit

# Dependency licenses in THIRD-PARTY.md: the rules are in about.toml, the template is in about.hbs
third-party:
    cargo about generate about.hbs -o THIRD-PARTY.md

# Bill of materials (SBOM) in the CycloneDX format: sbom/molva*.cdx.json
sbom:
    #!/usr/bin/env bash
    set -euo pipefail
    mkdir -p sbom
    cargo cyclonedx --format json --all --spec-version 1.5
    find crates -name '*.cdx.json' -exec mv {} sbom/ \;
    ls sbom

# Coverage as a map, not as a gate: an artifact for CI
cov:
    cargo llvm-cov --workspace --lcov --output-path coverage.lcov
