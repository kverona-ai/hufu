# Computer Use: Reference Study of LCU

| | |
|---|---|
| **Status** | Research snapshot for a planned capability (milestone M5) |
| **Verified on** | 2026-11-12 |
| **Feeds into** | [Requirements, section 7.9](../requirements/requirements.md#79-capability-routing), [ADR-0008](../adr/0008-computer-use-is-a-routable-capability.md), [Technical design, section 14](../technical/technical-design.md#14-capability-routing-planned) |

"Computer use" means an agent operating a desktop, a browser or another application through its
interface: reading the screen, clicking, typing. The project owner wants Hufu to be able to route
computer use either to a separately configured agent or to the executor's own agent, and named
**LCU** as the reference to study.

Evidence marks are those defined in the [agent landscape](agent-landscape.md). Everything here was
read from LCU's repository pages and documents; nothing was installed or run.

## 1. What LCU is

[LCU](https://github.com/amontlabs/lcu), by Amont Labs, describes itself as "Codex computer use,
decoupled from the Codex app". It takes the computer-use runtime that ships inside the official
ChatGPT desktop app and makes it usable from other agent harnesses. ✅

| Topic | Finding |
|---|---|
| Licence | MIT. The OpenAI app and its instructions "retain their own terms" ✅ |
| What it needs | The official ChatGPT desktop app installed locally. LCU does not download it and does not require a ChatGPT sign-in ✅ |
| Platforms | macOS on Apple Silicon; Linux on glibc (Ubuntu 24.04-compatible), ARM64 or x86-64, with X11 and D-Bus. Intel Macs, native Wayland and musl Linux are unsupported. Windows is a candidate ✅ |
| Other runtimes | The documents disagree on whether Python 3.12+ is required. Check at install time ⚠️ |
| Version | Documents refer to 0.8.x. Latest release not confirmed ❓ |

## 2. How it works

### 2.1 A runtime attached to a harness, not an agent

LCU runs **no model of its own**. It gives the calling harness's model two tools, `js` and
`js_reset`. The model writes JavaScript against the original runtime's API (list windows, take a
screenshot, click, type, and so on), and results come back unchanged: text as text, screenshots as
PNG images, audio as a path to a saved file. ✅

So with LCU, the model that plans and the model that operates the desktop are the same model: the
harness's. What LCU separates is the *runtime* from the *app it shipped in*. It has no notion of
handing computer use to a different agent. ✅

### 2.2 Attachment per harness

`lcu setup --agent <name>` registers LCU with a harness. Accepted names: `pi`, `codex`,
`claude-code`, `omp`, `hermes`. ✅

| Harness | Mechanism |
|---|---|
| Codex CLI | MCP registration through a generated relay, plus lifecycle hooks (Stop, Interrupt, SubagentStop). Needs a recent Codex CLI |
| Claude Code | MCP registration, a relay, and a small approval component; also installable as a Claude Code plugin |
| Pi | A native Pi extension |
| Oh My Pi, Hermes | Experimental plugins |
| DeepSeek Harness, Junie | Named as experimental on the repository page; no setup section found ❓ |

For hosts it does not know, LCU documents a shared client contract: connect, pass the original
instructions and images to the model, present approval requests, and report when a turn ends or is
interrupted. Some tools (`turn_ended` and others) are for the host only and must be hidden from the
model. ✅

### 2.3 Approval

Two layers, both required: ✅

1. **The harness** decides whether its agent may call LCU's tools at all. `--approval auto`
   pre-approves just these tools in the harness's own settings.
2. **LCU, per app**, decides whether the agent may touch a given application, in every permission
   mode. The choices are "allow this conversation", "always allow" and "deny". The model cannot see
   or answer the prompt.

Details that matter for a dispatcher:

- The app hosting the agent is never approved. Terminal apps are refused outright. Browsers and
  password managers carry a warning.
- `lcu apps` lists always-allowed apps. `lcu apps allow <App>` and `lcu apps revoke <App>` change
  the list and need Touch ID or a password on macOS, so an agent cannot grant itself access.
- **When nobody can answer, it fails closed.** A host that cannot present a request has the request
  cancelled. `codex exec` cannot use LCU under approval policy `never`. Pi's headless mode cancels
  approval requests.
- LCU says of its own cross-turn guard that it is "a guard against accidents, not a security
  boundary".

### 2.4 Lifetime, stop and concurrency

- Computer use belongs to the **turn** that started it and normally ends with it. Continuing across
  turns is an opt-in. A background sub-agent still running after the turn ended is refused. ✅
- Interrupting the harness sends the runtime's own interrupt. On Linux, a hung input call is bounded
  (30 seconds by default), after which pressed keys and buttons are released and the worker is
  restarted. ✅
- Within one LCU process on Linux, every call that changes focus or input goes through one queue.
  Whether two agents can use it at once is not stated. ❓

### 2.5 Sandbox

On Linux, the JavaScript kernel that runs the model's code is **not sandboxed by default**: the
model's code "can write files outside the workspace, use the network and start subprocesses". The
Codex relay does not pass on Codex's sandbox mode, so Codex's read-only sandbox does not restrict
it. ✅

## 3. What Hufu takes from LCU

| LCU idea | How Hufu uses it |
|---|---|
| The operating runtime is separate from the agent that plans | Hufu treats computer use as a **capability** that a profile provides, natively or through a runtime such as LCU, not as a kind of agent |
| Two approval layers | Two layers in Hufu as well: a per-task **grant** ("this job may use computer use") and an **app scope** ("only these applications"). Neither replaces the runtime's own per-app approval |
| Only a person can approve an app | Hufu never tries to answer or bypass a runtime's approval prompt. Apps must be allowed by the user beforehand |
| Fail closed when nobody can answer | A computer-use task whose apps are not already allowed is rejected before launch, with the command the user needs to run |
| Never operate the host app or a terminal | Hufu's app scope refuses terminals and the app hosting the agent, and requires browsers and password managers to be named explicitly |
| Computer use is scoped to a turn | In Hufu it is scoped to one job. It ends when the job ends |
| Interrupt reaches the runtime, input is released | `hufu cancel` on a computer-use job must reach the runtime's stop before the process is terminated |
| Results forwarded unchanged | Screenshots and logs stay as files in the job directory and are referenced from the result |
| Honest about what is not a boundary | Hufu's documents say plainly that computer use is not contained by permission tiers or worktrees |

## 4. What LCU does not do, and Hufu adds

LCU answers "how does *this* agent get computer use". It does not answer the project owner's
requirement, which is about **routing**:

| Route | Meaning | Relation to LCU |
|---|---|---|
| `executor` | The executor's own agent operates the desktop | The executor's harness has a computer-use runtime attached, for example through `lcu setup --agent codex`, or has one built in |
| `dedicated` | A separately configured agent operates the desktop on the executor's behalf | A second profile, for example "some model in Pi, with LCU attached", performs the step as a child job. Hufu supplies the hand-over, which LCU has no concept of |

The dedicated route is what lets a user pick the best operating model independently of the model
that is doing the coding.

## 5. Consequences and risks for Hufu

1. **A computer-use grant is wider than `full-access`.** With LCU on Linux the model's code is
   unsandboxed, and the harness's own sandbox setting does not apply to it. A grant must be treated
   as permission to act anywhere the user can.
2. **Headless use is restricted by design.** Because approvals fail closed, Hufu's CLI adapters that
   run with "never ask" policies cannot use LCU. Computer-use profiles will most likely need the ACP
   surface, where Hufu can at least relay a request to a waiting person, or apps allowed in advance.
   This must be tested per harness.
3. **Platform limits come from the runtime.** No Intel Macs, no Wayland. `hufu doctor` should report
   whether a computer-use runtime is present and usable on the machine.
4. **One display, one operator.** Hufu allows one computer-use job per display at a time, since
   nothing in LCU promises more.
5. **Terms of use.** LCU reuses a runtime that ships inside OpenAI's app. Whether that use is
   permitted is governed by OpenAI's terms, which were not reviewed here. Hufu will not bundle or
   install LCU; it only recognises a profile that already has it.
6. **LCU is young and moving.** Its own documents contradict each other in places. Treat every
   detail above as something to re-check when the milestone starts.

## 6. Open verification items

- [ ] Install LCU on a supported machine and confirm the tool list, result shapes and approval flow.
- [ ] Confirm which Hufu adapter surfaces can carry an LCU session headlessly (Codex over ACP,
      Claude Code, Pi in RPC mode).
- [ ] Confirm whether two harnesses can use the runtime at the same time.
- [ ] Survey agents with built-in computer use and how each is enabled non-interactively.
- [ ] Read LCU's provenance document and OpenAI's terms before recommending it.

## Sources

- <https://github.com/amontlabs/lcu>
- <https://raw.githubusercontent.com/amontlabs/lcu/main/README.md>
- <https://raw.githubusercontent.com/amontlabs/lcu/main/docs/ADAPTERS.md>
- <https://raw.githubusercontent.com/amontlabs/lcu/main/docs/INSTALLATION.md>
- <https://deepwiki.com/amontlabs/lcu> (machine-generated summary; used only as a pointer)
