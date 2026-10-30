#!/usr/bin/env python3
"""ENGINE-008 scorer : MBPP (extraction + sandbox docker isole).

Aucune inference modele. Le code teste est du code SYNTHETIQUE dans le selftest.
Backend : docker. Un seul backend, gele.
"""
import json
import os
import re
import subprocess
import tempfile

SCORER_ID = "mbpp_v1"
VERSION = "1"
SANDBOX_BACKEND = "docker"
INPUT_SCHEMA = ["item_id", "task_id", "response_text", "test_imports", "test_list"]
OUTPUT_SCHEMA = ["item_id", "status", "score"]

FENCE_PY = re.compile(r"```(?:python|py)?\s*\n(.*?)```", re.DOTALL)
FENCE_ANY = re.compile(r"```[^\n]*\n(.*?)```", re.DOTALL)

WALL_TIMEOUT_S = 30
CPU_LIMIT = "1.0"
MEM_LIMIT = "512m"
# MESURE : pids-limit 64 et 2 laissent os.fork() REUSSIR ; pids-limit 1 le bloque
# (BlockingIOError). C est la seule valeur qui rende la restriction REELLE.
PIDS_LIMIT = "1"
DOCKER_IMAGE = os.environ.get("ENGINE008_MBPP_IMAGE", "python:3.12-slim")


class MBPPError(Exception):
    pass


def extract_code(response_text):
    """Politique gelee d extraction. Rend (code, status)."""
    blocks = FENCE_PY.findall(response_text)
    if len(blocks) == 1:
        return blocks[0], None
    if len(blocks) > 1:
        return None, "EXTRACTION_ERROR"
    generic = FENCE_ANY.findall(response_text)
    if len(generic) == 1:
        return generic[0], None
    if len(generic) > 1:
        return None, "EXTRACTION_ERROR"
    return response_text, None


def _runner_source(candidate_code, test_imports, test_list):
    """Genere le script execute DANS le sandbox. Les tests sont dans l ordre upstream."""
    parts = []
    parts.append("import json, sys, traceback")
    parts.append("RESULT = {'status': None}")
    for imp in test_imports:
        parts.append("exec(%r)" % imp)
    parts.append("try:")
    parts.append("    exec(compile(%r, '<candidate>', 'exec'), globals())" % candidate_code)
    parts.append("except SyntaxError as e:")
    parts.append("    RESULT['status'] = 'SYNTAX_ERROR'; RESULT['detail'] = str(e); "
                 "print(json.dumps(RESULT)); sys.exit(0)")
    parts.append("except Exception as e:")
    parts.append("    RESULT['status'] = 'IMPORT_ERROR'; RESULT['detail'] = repr(e)[:300]; "
                 "print(json.dumps(RESULT)); sys.exit(0)")
    parts.append("try:")
    for t in test_list:
        parts.append("    exec(compile(%r, '<test>', 'exec'), globals())" % t)
    parts.append("except AssertionError as e:")
    parts.append("    RESULT['status'] = 'ASSERT_FAIL'; RESULT['detail'] = str(e)[:300]; "
                 "print(json.dumps(RESULT)); sys.exit(0)")
    parts.append("except Exception as e:")
    parts.append("    RESULT['status'] = 'RUNTIME_ERROR'; RESULT['detail'] = repr(e)[:300]; "
                 "print(json.dumps(RESULT)); sys.exit(0)")
    parts.append("RESULT['status'] = 'PASS'")
    parts.append("print(json.dumps(RESULT))")
    return "\n".join(parts)


def run_in_sandbox(candidate_code, test_imports, test_list, image=None):
    """Execute dans docker, isolation stricte. Rend (status, detail)."""
    img = image or DOCKER_IMAGE
    src = _runner_source(candidate_code, test_imports, test_list)
    with tempfile.TemporaryDirectory(prefix="e8-mbpp-") as td:
        p = os.path.join(td, "runner.py")
        with open(p, "w", encoding="utf-8", newline="\n") as f:
            f.write(src)
        os.chmod(p, 0o444)  # lecture seule
        cmd = [
            "docker", "run", "--rm",
            "--network", "none",              # reseau coupe
            "--cpus", CPU_LIMIT,              # limite CPU
            "--memory", MEM_LIMIT,            # limite memoire
            "--pids-limit", PIDS_LIMIT,       # limite de processus/fork
            "--read-only",                    # racine en lecture seule
            "--security-opt", "no-new-privileges",  # pas d elevation
            "--tmpfs", "/tmp:rw,noexec,nosuid,size=64m",
            "-v", "%s:/work/runner.py:ro" % p,  # seul montage, en lecture seule
            "-w", "/work",
            "--user", "65534:65534",
            "-e", "PYTHONDONTWRITEBYTECODE=1",
            "-e", "PYTHONHASHSEED=0",
            "--entrypoint", "python",
            img, "/work/runner.py",
        ]
        try:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=WALL_TIMEOUT_S)
        except subprocess.TimeoutExpired:
            # kill de tout l arbre : --rm + timeout suffisent, on force le nettoyage
            subprocess.run(["docker", "ps", "-q", "--filter", "ancestor=" + img],
                           capture_output=True, text=True)
            return "TIMEOUT", "wall timeout %ds" % WALL_TIMEOUT_S
        except Exception as e:
            return "SANDBOX_ERROR", repr(e)[:200]
        if r.returncode != 0 and not r.stdout.strip():
            return "SANDBOX_ERROR", (r.stderr.strip()[-300:] or "rc=%d" % r.returncode)
        out = r.stdout.strip().splitlines()
        if not out:
            return "SANDBOX_ERROR", "aucune sortie du runner"
        try:
            d = json.loads(out[-1])
        except Exception:
            return "SANDBOX_ERROR", "sortie non-JSON : %s" % out[-1][:120]
        return d.get("status", "SANDBOX_ERROR"), d.get("detail", "")


def score_item(item_id, task_id, response_text, test_imports, test_list, image=None):
    code, err = extract_code(response_text)
    if err:
        return dict(item_id=item_id, status=err, score=0)
    status, detail = run_in_sandbox(code, test_imports, test_list, image)
    return dict(item_id=item_id, status=status, score=1 if status == "PASS" else 0,
                detail=detail)


# ---------------------------------------------------------------- selftests
GOOD = "def add(a, b):\n    return a + b\n"
TESTS = ["assert add(1, 2) == 3", "assert add(-1, 1) == 0"]

CASES = [
    ("valid_implementation", "```python\n%s```" % GOOD, "PASS"),
    ("assertion_failure", "```python\ndef add(a, b):\n    return a - b\n```", "ASSERT_FAIL"),
    ("syntax_error", "```python\ndef add(a, b)\n    return a+b\n```", "SYNTAX_ERROR"),
    ("runtime_exception", "```python\ndef add(a, b):\n    raise ValueError('boom')\n```",
     "RUNTIME_ERROR"),
    ("infinite_loop", "```python\ndef add(a, b):\n    while True:\n        pass\n```", "TIMEOUT"),
    ("fork_attempt", "```python\nimport os\ndef add(a, b):\n    os.fork()\n    return a+b\n```",
     "RUNTIME_ERROR"),
    ("network_attempt",
     "```python\nimport socket\ndef add(a, b):\n    socket.create_connection(('1.1.1.1',80),2)\n"
     "    return a+b\n```", "RUNTIME_ERROR"),
    ("host_path_read",
     "```python\ndef add(a, b):\n    open('/etc/shadow').read()\n    return a+b\n```",
     "RUNTIME_ERROR"),
    ("write_outside_sandbox",
     "```python\ndef add(a, b):\n    open('/work/x','w').write('x')\n    return a+b\n```",
     "RUNTIME_ERROR"),
    ("multiple_fenced_blocks",
     "```python\n%s```\n```python\n%s```" % (GOOD, GOOD), "EXTRACTION_ERROR"),
    ("no_fence_uses_whole_response", GOOD, "PASS"),
]


def selftest():
    out = []
    def chk(n, c, d=""):
        out.append("%-52s %s %s" % (n, "PASS" if c else "FAIL", d)); return c
    ok = True
    # disponibilite du backend
    r = subprocess.run(["docker", "version", "--format", "{{.Server.Version}}"],
                       capture_output=True, text=True)
    chk("docker_available", r.returncode == 0, r.stdout.strip())
    if r.returncode != 0:
        return False, out
    # image presente ?
    r = subprocess.run(["docker", "image", "inspect", DOCKER_IMAGE],
                       capture_output=True, text=True)
    if r.returncode != 0:
        out.append("  image %s absente -> pull" % DOCKER_IMAGE)
        subprocess.run(["docker", "pull", "--quiet", DOCKER_IMAGE],
                       capture_output=True, text=True, timeout=600)
    for name, resp, want in CASES:
        res = score_item("t", 1, resp, [], TESTS)
        got = res["status"]
        # fork/path/write : le code peut lever PermissionError (RUNTIME_ERROR) ou
        # etre bloque par l isolation ; on accepte tout SAUF PASS.
        if name in ("fork_attempt", "network_attempt", "host_path_read",
                    "write_outside_sandbox"):
            cond = (got != "PASS")
        else:
            cond = (got == want)
        ok &= chk(name, cond, "attendu=%s observe=%s" % (want, got))
    return bool(ok), out


if __name__ == "__main__":
    import sys
    o, lines = selftest()
    print("\n".join(lines))
    print("MBPP_SANDBOX_SELFTEST=%s" % ("PASS" if o else "FAIL"))
    print("MBPP_SANDBOX_BACKEND=%s" % SANDBOX_BACKEND)
    sys.exit(0 if o else 1)
