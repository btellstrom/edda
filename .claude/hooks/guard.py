#!/usr/bin/env python3
"""A PreToolUse hook for Claude Code: blocks shell commands that could reach
the real campaign-manager stack, its data, or this machine's campaign-edit
config, so tests and experiments stay in tools/sandbox.sh, `make test` and
`make smoke`. Exit code 2 blocks the command and tells Claude why.

    .claude/hooks/guard.py --self-test     checks the rules below
"""

import json
import re
import sys

# Where a command can start: the beginning, after ; & | ( or a newline, in
# $( ) or backticks, after `exec`, `env [VAR=x ...]`, `sh -c`, or a quote
# (a command handed to tmux or ssh), and after VAR=value assignments.
COMMAND_START = (
    r"(?:^|[;&|(\n`]|\$\(|\bexec\s+|\benv\s+(?:-\S+\s+(?:[A-Z_][A-Z0-9_]*\s+)?)*|\b(?:sh|bash)\s+-c\s+|[\"'])"
    r"\s*(?:\w+=\S*\s+)*"
)

RULES = [
    (
        COMMAND_START + r"(?:\S*/)?campaign-edit(?![\w./-])",
        "runs campaign-edit outside the sandbox (it would use this machine's "
        "config and the real servers); use `tools/sandbox.sh run|tmux`",
    ),
    (
        r"\bcampaign-manager-(?:postgres|auth|characters|combat|monsters|seed-monsters|register-combat)-\d",
        "touches a container of the real stack",
    ),
    (
        r"\b(?:localhost|127\.0\.0\.1):(?:5432|300[0-3])\b",
        "talks to the real stack's ports (5432, 3000-3003)",
    ),
    (
        r"\bmake(?:\s+-\S+)*(?:\s+\w+=\S*)*\s+(?:up|down|logs|campaign|delete-campaign|server|install)\b",
        "a make target that builds, starts, stops or changes the real stack "
        "(or installs over the user's campaign-edit)",
    ),
    (
        r"\b(?:podman|docker)(?:[ -]compose|\s+compose)\b(?![^;&|\n]*(?:-p\s+|--project-name[ =])campaign-manager-(?:smoke|sandbox))",
        "compose without an isolated project name acts on the real stack",
    ),
    (
        r"\bdocker-compose\b",
        "compose without an isolated project name acts on the real stack",
    ),
    (
        r"\b(?:podman|docker)\s+(?:build|tag|rmi|push|image\s+rm)\b[^;&|\n]*localhost/campaign-manager(?![-\w])",
        "builds or changes the real stack's image (localhost/campaign-manager)",
    ),
    (
        r"\b(?:podman|docker)\s+(?:system\s+prune|(?:volume|image|container)\s+(?:prune|rm)\b[^;&|\n]*campaign-manager_)",
        "deletes containers, images or volumes that may be the user's",
    ),
    (
        r"\b(?:podman|docker)\s+(?:system|volume|image|container)\s+prune\b",
        "prunes containers, images or volumes, which may be the user's",
    ),
    (
        r"\.config/campaign-manager\b",
        "reads or changes this machine's campaign-edit config",
    ),
]


def strip_heredocs(command):
    """Drops here-document bodies: text fed to a program (a commit message,
    a file being written), not commands."""
    out, delimiter = [], None
    for line in command.split("\n"):
        if delimiter is not None:
            if line.strip() == delimiter:
                delimiter = None
            continue
        out.append(line)
        found = re.search(r"<<-?\s*(['\"]?)(\w+)\1", line)
        if found:
            delimiter = found.group(2)
    return "\n".join(out)


def problem(command):
    text = strip_heredocs(command)
    for pattern, reason in RULES:
        if re.search(pattern, text):
            return reason
    return None


SELF_TEST = {
    # Blocked.
    "campaign-edit list": True,
    "~/.cargo/bin/campaign-edit scanlan show": True,
    "target/debug/campaign-edit list": True,
    "cd x && campaign-edit fight": True,
    "FOO=1 campaign-edit list": True,
    "env -u X campaign-edit list": True,
    "tmux new-session -d -s f \"source env; campaign-edit fight --new Test\"": True,
    "podman exec campaign-manager-postgres-1 psql -U postgres": True,
    "curl http://127.0.0.1:3002/health": True,
    "psql postgres://postgres:postgres@localhost:5432/auth": True,
    "make up": True,
    "make -s delete-campaign ID=1 YES=1": True,
    "make install": True,
    "podman compose up -d": True,
    "podman compose logs seed-monsters": True,
    "podman build -t localhost/campaign-manager .": True,
    "podman tag abc localhost/campaign-manager:latest": True,
    "podman system prune -f": True,
    "podman volume rm campaign-manager_postgres-data": True,
    "ls ~/.config/campaign-manager": True,
    # Allowed.
    "tools/sandbox.sh run list": False,
    "tools/sandbox.sh tmux fight fight --new Test": False,
    "cargo build -p campaign-edit": False,
    "cargo install --path crates/campaign-edit --root /tmp/x": False,
    "cargo clippy --workspace -- -D warnings && cargo test -p campaign-edit": False,
    "grep -rn foo crates/campaign-edit/src": False,
    "make test": False,
    "make check": False,
    "make smoke": False,
    "make srd": False,
    "curl http://127.0.0.1:23002/health": False,
    "curl http://127.0.0.1:13000/health": False,
    "podman compose -p campaign-manager-smoke down -v": False,
    "podman build --network host -t localhost/campaign-manager-smoke .": False,
    "podman run --rm -d --name test-pg -p 127.0.0.1:55434:5432 postgres": False,
    "git commit -F - <<'EOF'\nTeach campaign-edit to fight\n\ncampaign-edit fight --new X\nEOF": False,
    "python3 - <<'EOF'\nprint('make up')\nEOF": False,
}


def self_test():
    failed = [
        (command, expected)
        for command, expected in SELF_TEST.items()
        if (problem(command) is not None) != expected
    ]
    for command, expected in failed:
        print(f"{'should block' if expected else 'should allow'}: {command!r}")
    print(f"{len(SELF_TEST) - len(failed)}/{len(SELF_TEST)} as expected")
    return 1 if failed else 0


def main():
    if sys.argv[1:] == ["--self-test"]:
        return self_test()
    event = json.load(sys.stdin)
    command = event.get("tool_input", {}).get("command", "")
    reason = problem(command)
    if reason is None:
        return 0
    print(
        f"Blocked by .claude/hooks/guard.py: this {reason}. Tests and experiments "
        "must never reach the user's real campaign-manager stack or data: use "
        "tools/sandbox.sh, `make test` or `make smoke`, or ask the user to run "
        "the command themselves.",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
