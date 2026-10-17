#!/usr/bin/env python3
"""A PreToolUse hook for Claude Code: blocks shell commands that could reach
the real stack (named edda, or campaign-manager before the rename), its
data, or this machine's edda (campaign-edit before)
config, so tests and experiments stay in tools/sandbox.sh, `make test` and
`make smoke`. Exit code 2 blocks the command and tells Claude why.

The command is read the way the shell reads it: split into words (quotes
and escapes honoured) and into simple commands (at ; | && and so on).
Rules about what a command does (runs edda, `make up`, compose)
look at each command's program and arguments, past wrappers such as `env`,
`sudo`, `timeout` or `xargs`, and also inside the text that gets run as a
command in turn: $( ), backticks, <( ), `sh -c`, `eval`, and what tmux,
ssh, `watch`, `find -exec` and `su`/`script -c` are given to run. Quoted
text that's only an argument (a grep pattern, an echo, a commit message)
is just text. Rules about data (the real stack's containers, ports and
config) apply anywhere in the command, quoted or not, except in
here-document bodies.

Variables are followed: `E=target/debug/edda; $E list` runs
edda, and so does a shell function wrapping it (its body counts
where it's defined). It guards against mistakes, not deliberate evasion:
what a script file runs, a script piped into `sh`, a variable set
elsewhere, or an alias get past it.

    .claude/hooks/guard.py --self-test     checks the rules below
"""

import json
import os
import re
import shlex
import sys

EDDA = (
    "runs edda outside the sandbox (it would use this machine's "
    "config and the real servers); use `tools/sandbox.sh run|tmux`"
)
MAKE = (
    "a make target that builds, starts, stops or changes the real stack "
    "(or installs over the user's edda)"
)
COMPOSE = "compose without an isolated project name acts on the real stack"
IMAGE = "builds or changes the real stack's image (localhost/edda)"
PRUNE = "prunes containers, images or volumes, which may be the user's"
DELETE = "deletes containers, images or volumes that may be the user's"
SERVICE = (
    "runs a service binary with its defaults, which point at the real stack's "
    "database and ports; set DATABASE_URL (and AUTH_SERVICE_URL to serve) in "
    "the command, or use tools/sandbox.sh"
)
BACKUP = "backs up or restores the real stack's data (the smoke test checks the scripts)"
SERVICES = {"auth-service", "character-service", "combat-service", "monster-service"}

# Make targets that act on the real stack, or install over edda.
REAL_STACK_TARGETS = {
    "up", "down", "logs", "campaign", "delete-campaign", "server", "install", "backup", "restore",
}
# Compose projects of their own: the smoke test's and the sandbox's.
ISOLATED_PROJECTS = ("edda-smoke", "edda-sandbox")

# Rules about data: matched against the whole command (here-document bodies
# left out), wherever it appears.
DATA_RULES = [
    (
        r"\b(?:edda|campaign-manager)-(?:postgres|auth|characters|combat|monsters|seed-monsters|register-combat)-\d",
        "touches a container of the real stack",
    ),
    (
        r"\b(?:localhost|127\.0\.0\.1):(?:5432|300[0-3])\b",
        "talks to the real stack's ports (5432, 3000-3003)",
    ),
    (
        r"\.config/(?:edda|campaign-manager)\b",
        "reads or changes this machine's edda config",
    ),
]

# --- Reading the shell ------------------------------------------------------

OPERATORS = ("&&", "||", ";;", "|&", ";", "|", "&", "(", ")", "\n")
REDIRECT = re.compile(r"&>>|&>|<<<|<<-|<<|>>|>&|<&|>\||<>|<|>")
ASSIGNMENT = re.compile(r"^[A-Za-z_][A-Za-z0-9_]*=")


def closing_paren(text, start):
    """The index of the `)` matching the `(` at `start`, skipping quoted
    text; the end of `text` if there's none."""
    depth, i = 0, start
    while i < len(text):
        c = text[i]
        if c == "\\":
            i += 2
            continue
        if c == "'":
            end = text.find("'", i + 1)
            i = len(text) if end < 0 else end + 1
            continue
        if c == '"':
            i = closing_quote(text, i + 1) + 1
            continue
        if c == "(":
            depth += 1
        elif c == ")":
            depth -= 1
            if depth == 0:
                return i
        i += 1
    return len(text)


def closing_quote(text, start):
    """The index of the `"` that closes a double-quoted string starting at
    `start`; the end of `text` if there's none."""
    i = start
    while i < len(text):
        c = text[i]
        if c == "\\":
            i += 2
        elif c == '"':
            return i
        elif text.startswith("$(", i):
            i = closing_paren(text, i + 1) + 1
        elif c == "`":
            end = closing_backtick(text, i + 1)
            i = end + 1
        else:
            i += 1
    return len(text)


def closing_backtick(text, start):
    i = start
    while i < len(text):
        if text[i] == "\\":
            i += 2
        elif text[i] == "`":
            return i
        else:
            i += 1
    return len(text)


def expansions(text):
    """The command substitutions in `text` as the shell expands it inside
    double quotes (or in an unquoted here-document)."""
    found, i = [], 0
    while i < len(text):
        if text[i] == "\\":
            i += 2
        elif text.startswith("$(", i):
            end = closing_paren(text, i + 1)
            found.append(text[i + 2 : end])
            i = end + 1
        elif text[i] == "`":
            end = closing_backtick(text, i + 1)
            found.append(text[i + 1 : end])
            i = end + 1
        else:
            i += 1
    return found


class Script:
    """Shell code read into simple commands (each a list of words, with
    redirections left out), the code it runs in turn (command and process
    substitutions), and its text without here-document bodies."""

    def __init__(self, text):
        self.text = text
        self.commands = []
        self.nested = []
        self.bodies = []  # (start, end) of each here-document body
        self.read()

    def read(self):
        text, i = self.text, 0
        command, word, quoted = [], None, False
        skip_word = False  # the next word is a redirection's target
        heredocs = []  # (delimiter, strip tabs, expands) waiting for a newline
        delimiter_next = None  # a `<<`'s delimiter is the next word

        def flush():
            nonlocal word, quoted, skip_word, delimiter_next
            if word is None:
                return
            value = "".join(word)
            if delimiter_next is not None:
                heredocs.append((value, delimiter_next == "<<-", not quoted))
                delimiter_next = None
            elif skip_word:
                skip_word = False
            else:
                command.append(value)
            word, quoted = None, False

        def end_command():
            nonlocal command
            flush()
            if command:
                self.commands.append(command)
            command = []

        def start():
            nonlocal word
            if word is None:
                word = []

        while i < len(text):
            c = text[i]
            if c in " \t":
                flush()
                i += 1
            elif c == "\\":
                if text.startswith("\\\n", i):
                    i += 2
                    continue
                start()
                word.append(text[i + 1 : i + 2])
                quoted = True
                i += 2
            elif c == "#" and word is None:
                end = text.find("\n", i)
                i = len(text) if end < 0 else end
            elif c == "'":
                start()
                quoted = True
                end = text.find("'", i + 1)
                end = len(text) if end < 0 else end
                word.extend(text[i + 1 : end])
                i = end + 1
            elif c == '"':
                start()
                quoted = True
                end = closing_quote(text, i + 1)
                inner = text[i + 1 : end]
                self.nested.extend(expansions(inner))
                word.extend(re.sub(r'\\([$`"\\])', r"\1", inner))
                i = end + 1
            elif text.startswith("$(", i):
                end = closing_paren(text, i + 1)
                self.nested.append(text[i + 2 : end])
                start()
                word.extend("$(…)")
                i = end + 1
            elif c == "`":
                end = closing_backtick(text, i + 1)
                self.nested.append(text[i + 1 : end])
                start()
                word.extend("`…`")
                i = end + 1
            elif c in "<>" and text.startswith("(", i + 1):
                # Process substitution: <(command) or >(command).
                end = closing_paren(text, i + 1)
                self.nested.append(text[i + 2 : end])
                start()
                word.extend("<(…)")
                i = end + 1
            elif c in "<>" or text.startswith("&>", i):
                # A file descriptor number before it belongs to it.
                if word is not None and "".join(word).isdigit() and not quoted:
                    word, quoted = None, False
                flush()
                op = REDIRECT.match(text, i).group()
                i += len(op)
                if op in ("<<", "<<-"):
                    delimiter_next = op
                elif op not in (">&", "<&") or not re.match(r"\d|-", text[i : i + 1]):
                    skip_word = True
                else:
                    # 2>&1, <&- : the target is part of the operator.
                    i += len(re.match(r"\d+|-", text[i:]).group())
            else:
                op = next((o for o in OPERATORS if text.startswith(o, i)), None)
                if op is None:
                    start()
                    word.append(c)
                    i += 1
                    continue
                end_command()
                i += len(op)
                if op == "\n" and heredocs:
                    i = self.skip_bodies(i, heredocs)
                    heredocs = []
        end_command()

    def skip_bodies(self, i, heredocs):
        """Skips the here-document bodies that start at `i`, noting the code
        that unquoted ones run; where the shell carries on."""
        text = self.text
        for delimiter, strip_tabs, expands in heredocs:
            start = i
            while i < len(text):
                end = text.find("\n", i)
                end = len(text) if end < 0 else end
                line = text[i:end]
                i = min(end + 1, len(text))
                if (line.lstrip("\t") if strip_tabs else line).strip() == delimiter:
                    break
                if expands:
                    self.nested.extend(expansions(line))
            self.bodies.append((start, i))
        return i

    def visible_text(self):
        """The text without its here-document bodies: what's typed, not
        what's fed to a program."""
        out, last = [], 0
        for start, end in self.bodies:
            out.append(self.text[last:start])
            last = end
        out.append(self.text[last:])
        return "".join(out)


# --- What each command runs -------------------------------------------------

SHELLS = {"sh", "bash", "zsh", "dash", "ksh"}
# Words that come before a command without being it.
RESERVED = {"if", "then", "elif", "else", "do", "while", "until", "!", "{", "}", "time", "fi", "done"}
# Programs that run the rest of their arguments, with their options that take
# a value.
WRAPPERS = {
    "exec": set(),
    "command": set(),
    "builtin": set(),
    "nohup": set(),
    "setsid": set(),
    "sudo": {"-u", "-g", "-C", "-h", "-p", "-U", "-r", "-t", "-D"},
    "doas": {"-u", "-C"},
    "nice": {"-n"},
    "ionice": {"-c", "-n", "-p"},
    "stdbuf": {"-i", "-o", "-e"},
    "env": {"-u", "--unset", "-C", "--chdir"},
    "timeout": {"-s", "--signal", "-k", "--kill-after"},
    "xargs": {"-I", "-n", "-P", "-L", "-d", "-E", "-s", "-a", "--max-args", "--max-procs",
              "--delimiter", "--arg-file", "--max-lines", "--replace", "--eof"},
}
TMUX_RUNNERS = {
    "new-session", "new", "new-window", "neww", "split-window", "splitw",
    "respawn-pane", "respawnp", "respawn-window", "respawnw", "run-shell", "run",
    "display-popup", "popup", "send-keys", "send", "pipe-pane", "pipep", "if-shell", "if",
}
SSH_OPTIONS = {"-p", "-i", "-l", "-o", "-F", "-J", "-L", "-R", "-D", "-E", "-b", "-c",
               "-m", "-O", "-Q", "-S", "-W", "-w", "-B", "-I", "-P"}


def program(argv, nested):
    """`argv` from its program on, past assignments, reserved words and
    wrappers (`env -S` code goes to `nested`)."""
    i = 0
    while i < len(argv):
        word = argv[i]
        name = os.path.basename(word)
        if ASSIGNMENT.match(word) or word in RESERVED:
            i += 1
            continue
        if name not in WRAPPERS:
            return argv[i:]
        valued = WRAPPERS[name]
        i += 1
        while i < len(argv) and argv[i].startswith("-") and argv[i] != "-":
            option = argv[i]
            if name == "env" and option in ("-S", "--split-string"):
                nested.extend(argv[i + 1 : i + 2])
                i += 2
            elif name == "env" and option.startswith(("-S", "--split-string=")):
                nested.append(option.split("=", 1)[1] if "=" in option else option[2:])
                i += 1
            elif option == "--":
                i += 1
                break
            else:
                i += 2 if option in valued else 1
        if name == "timeout":
            i += 1  # the duration
    return []


def runs(argv, nested):
    """Notes the code `argv`'s program runs as a command in turn."""
    name, args = os.path.basename(argv[0]), argv[1:]
    if name in SHELLS:
        for k, arg in enumerate(args):
            if arg.startswith("-") and not arg.startswith("--") and "c" in arg[1:]:
                nested.extend(args[k + 1 : k + 2])
                break
            if not arg.startswith("-"):
                break
    elif name == "eval":
        nested.append(" ".join(args))
    elif name in ("su", "runuser", "script"):
        for k, arg in enumerate(args):
            if arg in ("-c", "--command"):
                nested.extend(args[k + 1 : k + 2])
            elif arg.startswith("--command="):
                nested.append(arg.split("=", 1)[1])
    elif name == "tmux":
        subcommand = next((k for k, arg in enumerate(args) if not arg.startswith("-")), None)
        if subcommand is not None and args[subcommand] in TMUX_RUNNERS:
            rest = args[subcommand + 1 :]
            nested.extend(rest)
            nested.append(" ".join(rest))
    elif name == "ssh":
        k = 0
        while k < len(args) and args[k].startswith("-"):
            k += 2 if args[k] in SSH_OPTIONS else 1
        if k + 1 < len(args):
            nested.append(" ".join(args[k + 1 :]))
    elif name == "watch":
        k = 0
        while k < len(args) and args[k].startswith("-"):
            k += 2 if args[k] in ("-n", "--interval", "-d", "--differences") else 1
        nested.append(" ".join(args[k:]))
    elif name == "find":
        for k, arg in enumerate(args):
            if arg in ("-exec", "-execdir", "-ok", "-okdir"):
                end = next((j for j in range(k + 1, len(args)) if args[j] in (";", "+")), len(args))
                nested.append(" ".join(shlex.quote(a) for a in args[k + 1 : end]))


VARIABLE = re.compile(r"\$(?:\{(\w+)\}|(\w+))")
DECLARES = {"export", "declare", "local", "readonly", "typeset"}


def expand(word, variables):
    """`word` with the variables known so far filled in."""
    return VARIABLE.sub(lambda m: variables.get(m.group(1) or m.group(2), m.group(0)), word)


def remember(argv, variables):
    """Notes the variables `argv` sets: `E=…`, or `export E=…` and the like."""
    words = argv[1:] if argv and argv[0] in DECLARES else argv
    for word in words:
        if ASSIGNMENT.match(word):
            name, value = word.split("=", 1)
            variables[name] = value
        elif argv[0] not in DECLARES:
            break


def commands(text, variables=None, depth=0):
    """Every command `text` runs, as its program and arguments with the
    variables set by then, including those in the code it runs in turn.
    `variables` are those set so far."""
    variables = {} if variables is None else variables
    script = Script(text)
    nested = list(script.nested)
    found = []
    for argv in script.commands:
        argv = [expand(word, variables) for word in argv]
        remember(argv, variables)
        argv = program(argv, nested)
        if argv:
            found.append((argv, dict(variables)))
            runs(argv, nested)
    if depth < 8:
        for code in nested:
            found.extend(commands(code, variables, depth + 1))
    return found


# --- The rules --------------------------------------------------------------


def make_targets(args):
    targets, k = [], 0
    valued = {"-C", "-f", "-I", "-o", "-W", "--directory", "--file", "--makefile",
              "--include-dir", "--old-file", "--assume-old", "--what-if", "--assume-new", "--new-file"}
    while k < len(args):
        arg = args[k]
        if arg in valued:
            k += 2
            continue
        if arg == "-j" and k + 1 < len(args) and args[k + 1].isdigit():
            k += 2
            continue
        if not arg.startswith("-") and not ASSIGNMENT.match(arg):
            targets.append(arg)
        k += 1
    return targets


def isolated_project(args):
    for k, arg in enumerate(args):
        if arg in ("-p", "--project-name"):
            project = args[k + 1] if k + 1 < len(args) else ""
        elif arg.startswith("--project-name="):
            project = arg.split("=", 1)[1]
        elif arg.startswith("-p") and not arg.startswith("--") and len(arg) > 2:
            project = arg[2:]
        else:
            continue
        if project.startswith(ISOLATED_PROJECTS):
            return True
    return False


def real_image(arg):
    image = arg.split("=", 1)[1] if arg.startswith("-") and "=" in arg else arg
    return any(image == name or image.startswith(name + ":")
               for name in ("localhost/edda", "localhost/campaign-manager"))


def container_problem(args):
    """What's wrong with a podman or docker command, if anything."""
    k = 0
    valued = {"--url", "-c", "--connection", "--log-level", "--root", "--runroot",
              "--storage-driver", "-H", "--host", "--context", "--config"}
    while k < len(args) and args[k].startswith("-"):
        k += 2 if args[k] in valued else 1
    if k >= len(args):
        return None
    sub, rest = args[k], args[k + 1 :]
    action = rest[0] if rest else ""
    if sub == "compose":
        return None if isolated_project(rest) else COMPOSE
    if sub in ("build", "tag", "rmi", "push") or (
        sub == "image" and action in ("build", "tag", "rm", "push")
    ):
        if any(real_image(arg) for arg in rest):
            return IMAGE
    if (sub == "system" and action == "prune") or (
        sub in ("volume", "image", "container") and action == "prune"
    ):
        return PRUNE
    if sub in ("rm", "rmi") or (sub in ("volume", "image", "container") and action == "rm"):
        if any(arg.startswith(("edda_", "campaign-manager_")) for arg in rest):
            return DELETE
    return None


def service_problem(name, args, variables):
    """Running a service binary: fine if the command says where its data
    and the auth service are, rather than leaving the defaults (the real
    stack's)."""
    if args and args[0] in ("public-key", "--help", "-h", "help", "--version", "-V"):
        return None
    if "DATABASE_URL" not in variables:
        return SERVICE
    serves = not args or args[0] == "serve" or args[0].startswith("-")
    if serves and name != "auth-service" and "AUTH_SERVICE_URL" not in variables:
        return SERVICE
    return None


def command_problem(argv, variables):
    name, args = os.path.basename(argv[0]), argv[1:]
    if name in ("edda", "campaign-edit"):
        return EDDA
    if name in SERVICES:
        return service_problem(name, args, variables)
    if name in ("backup.sh", "restore.sh"):
        return BACKUP
    if name in ("make", "gmake") and REAL_STACK_TARGETS.intersection(make_targets(args)):
        return MAKE
    if name in ("docker-compose", "podman-compose"):
        return None if isolated_project(args) else COMPOSE
    if name in ("podman", "docker"):
        return container_problem(args)
    return None


def problem(command):
    for argv, variables in commands(command):
        reason = command_problem(argv, variables)
        if reason:
            return reason
    text = Script(command).visible_text()
    for pattern, reason in DATA_RULES:
        if re.search(pattern, text):
            return reason
    return None


SELF_TEST = {
    # Blocked.
    "edda list": True,
    "~/.cargo/bin/edda scanlan show": True,
    "target/debug/edda list": True,
    '"edda" list': True,
    "cd x && edda fight": True,
    "FOO=1 edda list": True,
    "env -u X edda list": True,
    "env -S 'edda list'": True,
    "sudo -u ben edda list": True,
    "timeout 5 edda list": True,
    "xargs -I{} edda {} show < names": True,
    "if edda list; then echo ok; fi": True,
    "{ edda list; }": True,
    "(edda list)": True,
    'echo "$(edda list)"': True,
    "x=`edda list`": True,
    "cat <(edda list)": True,
    "bash -c 'cd x && edda list'": True,
    'sh -lc "edda list"': True,
    'eval "edda list"': True,
    "tmux new-session -d -s f \"source env; edda fight --new Test\"": True,
    "tmux new -d -s f edda fight": True,
    'tmux send-keys -t f "edda fight" Enter': True,
    "ssh host edda list": True,
    "watch -n 1 edda list": True,
    "find . -name '*.json' -exec edda import {} \\;": True,
    "cat <<EOF\n$(edda list)\nEOF": True,
    "E=target/debug/edda; $E list": True,
    'B=target/debug; E=$B/edda; "$E" fight --new X': True,
    'B=/x/edda\nce() { $B "$@"; }\nce list': True,
    "export E=~/.cargo/bin/edda && ${E} list": True,
    'E=target/debug/edda; tmux new -d -s f "$E fight"': True,
    "podman exec edda-postgres-1 psql -U postgres": True,
    "curl http://127.0.0.1:3002/health": True,
    'curl "http://localhost:3000/characters"': True,
    "psql postgres://postgres:postgres@localhost:5432/auth": True,
    "make up": True,
    "make -s delete-campaign ID=1 YES=1": True,
    "make -C . install": True,
    "make install": True,
    "make backup": True,
    "make restore FROM=backups/x": True,
    "tools/restore.sh backups/x": True,
    "target/debug/character-service backup /tmp/x.db": True,
    "target/debug/combat-service delete-campaign 1": True,
    "DATABASE_URL=sqlite:///tmp/c.db target/debug/character-service": True,
    "target/debug/monster-service": True,
    "podman compose up -d": True,
    "podman compose logs seed-monsters": True,
    "docker-compose down": True,
    "podman build -t localhost/edda .": True,
    "podman tag abc localhost/edda:latest": True,
    "podman system prune -f": True,
    "podman volume prune": True,
    "podman volume rm edda_postgres-data": True,
    "ls ~/.config/edda": True,
    # The stack and client from before the rename.
    "campaign-edit list": True,
    "~/.cargo/bin/campaign-edit fight": True,
    "podman exec campaign-manager-postgres-1 psql -U postgres": True,
    "podman tag abc localhost/campaign-manager:latest": True,
    "podman volume rm campaign-manager_postgres-data": True,
    "ls ~/.config/campaign-manager": True,
    "podman compose -p campaign-manager down": True,
    # Allowed.
    "tools/sandbox.sh run list": False,
    "DATABASE_URL=sqlite:///tmp/m.db target/debug/monster-service import x.json": False,
    "export DATABASE_URL=sqlite:///tmp/c.db; target/debug/character-service backup /tmp/b.db": False,
    "DATABASE_URL=sqlite:///tmp/c.db AUTH_SERVICE_URL=http://127.0.0.1:1 character-service": False,
    "target/debug/combat-service public-key": False,
    "target/debug/monster-service --help": False,
    "E=target/debug/edda; ls -la $E && stat ${E}": False,
    "CMD=ls; $CMD target/debug": False,
    "tools/sandbox.sh tmux fight fight --new Test": False,
    "SANDBOX_AS=Laura tools/sandbox.sh run vex show": False,
    "cargo build -p edda": False,
    "cargo install --path crates/edda --root /tmp/x": False,
    "cargo clippy --workspace -- -D warnings && cargo test -p edda": False,
    "grep -rn foo crates/edda/src": False,
    'grep -n "\\[Monsters\\](#monsters)\\|edda vex\\b" README.md': False,
    "grep -rn 'edda list' docs | head": False,
    "rg 'echo $(edda list)' README.md": False,
    'echo "run edda list to see them"': False,
    "printf '%s\\n' 'edda fight'": False,
    'git commit -m "Teach edda to fight; make up and podman compose are untouched"': False,
    'git log --grep="podman compose up"': False,
    "sed -n '/edda/p' README.md": False,
    "ls target/debug/edda && echo edda | wc -c": False,
    "make test": False,
    "make check": False,
    "make smoke": False,
    "make srd": False,
    "curl http://127.0.0.1:23002/health": False,
    "curl http://127.0.0.1:13000/health": False,
    "podman compose -p edda-smoke down -v": False,
    "podman volume rm edda-smoke_postgres-data": False,
    "podman compose --project-name=edda-sandbox ps": False,
    "podman build --network host -t localhost/edda-smoke .": False,
    "podman run --rm -d --name test-pg -p 127.0.0.1:55434:5432 postgres": False,
    "podman images 2>&1 | grep edda": False,
    "git commit -F - <<'EOF'\nTeach edda to fight\n\nedda fight --new X\nEOF": False,
    "python3 - <<'EOF'\nprint('make up')\nprint('127.0.0.1:3002')\nEOF": False,
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
        "must never reach the user's real edda stack or data: use "
        "tools/sandbox.sh, `make test` or `make smoke`, or ask the user to run "
        "the command themselves.",
        file=sys.stderr,
    )
    return 2


if __name__ == "__main__":
    sys.exit(main())
