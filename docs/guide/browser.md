# Browser

The Browser surface runs a real page inside the Workbench and lets agents drive
it. Use it when the evidence for a change lives in something rendered: a
frontend fix, a dev-server route, a docs page an agent needs to read.

The page runs in a separate browser process with its own isolated profile, so
an agent-driven session never inherits your cookies or signed-in state.

## Open a Browser

Open a new tab and choose **Browser** from the surface launcher. Unlike Agent
Session, Browser does not need anything selected first — it creates its own
session when you open it.

The tab shows an address bar, back, forward, and reload controls, a load-state
chip, and a short reference such as `browser:1`. That short reference is how
you target the same page from the command line.

A new Browser opens on your dev server when it can find one, rather than on a
blank page. Wardian reads the port out of the workspace's `vite.config.*`,
`package.json`, or `.env`, falls back to the ports frameworks commonly use, and
opens the first one that is actually listening. Selecting a single agent first
scopes that to the agent's workspace.

It is a guess, and the address bar overrules it.

## Drive the Page Yourself

Type a URL in the address bar and press `Enter`. Click, scroll, and type in the
page as usual. `Ctrl`/`Cmd` chords stay with the Workbench, so a focused page
cannot swallow tab switching or the command palette.

A hidden tab stops streaming while the page keeps running, so a background
browser costs nothing to leave open.

**One driver at a time.** If you open the same session in two panes, the first
one drives and the rest mirror it read-only, with their controls visibly
disabled. This prevents two views of one page from fighting over it. Close the
driving pane and the lease moves to a pane that is still watching, which takes
its controls back without being reopened.

The page is rendered at the size of the pane showing it, so resizing the pane
relays the page out rather than rescaling a picture of it. Text stays sharp,
and the site's own responsive breakpoints answer to the pane you sized.

**Windows a page opens.** A pane shows one page, so a popup — a
`target="_blank"` link, a "Sign in with…" window — takes the place of the page
that opened it, and a **Close popup** button appears next to the address. Use
it to go back: the popup has its own history, and there is no Back entry for
the page behind it. A popup that closes itself, which is what most sign-in
windows do, returns you there on its own. Returning does not wait for a title
or load reply from the closed window, and the opener's pending requests continue.

**Dialogs.** When a page calls `alert`, `confirm`, or `prompt`, or asks you to
confirm leaving, the page stops until you answer, and the surface shows the
message with the buttons that dialog has. Nothing else on the page responds
until then — that is how dialogs work in any browser. A session no pane is
watching answers its own dialogs so an agent driving it is never stuck:
"leave this page" is accepted, everything else is dismissed.

## Drive the Page from an Agent

Agents use `wardian browser`, which acts on exactly the same session you are
watching. Anything the agent does appears live in the surface.

```bash
wardian browser open                      # the workspace's dev server, if one is up
wardian browser open --blank              # no page at all
wardian browser open https://localhost:5173
wardian browser list
wardian browser browser:1 navigate https://localhost:5173/settings
wardian browser browser:1 snapshot
wardian browser browser:1 fill e4 "hello"
wardian browser browser:1 click e7
wardian browser browser:1 wait --text "Saved"
wardian browser browser:1 screenshot ./evidence.png
```

PowerShell uses the same commands.

`snapshot` returns the page's interactive elements with short references
(`e1`, `e2`, …) that later commands act on. If the page changes underneath
them — a navigation, a re-rendered list — the reference is refused with an
explicit reason rather than acting on the wrong element. When that happens,
take a new snapshot and use the new references.

Add `--json` after `browser` for machine-readable output:

```bash
wardian browser --json browser:1 snapshot
```

A session opened with `wardian browser open` appears as a tab in the app
automatically. Add `--detached` when an agent wants a browser without one.

## See What the Page Did

Rendering tells you what a page shows. When a form submit quietly fails, the
page looks the same either way and the evidence is in the request.

```bash
wardian browser browser:1 network --failed
wardian browser browser:1 network --filter /api/ --limit 20
wardian browser browser:1 network 12345.7 --body
wardian browser browser:1 console --level error
```

The request list records everything since the session opened and is not cleared
by navigation, so the page load itself is in there too. `--clear` empties it.

Cookies and web storage belong to this session's isolated profile, so reading
and changing them never touches your own browser:

```bash
wardian browser browser:1 cookies
wardian browser browser:1 cookies set sid abc --http-only
wardian browser browser:1 storage local
wardian browser browser:1 storage local set theme dark
```

Downloaded files land in a directory of their own and stay there after the
session closes:

```bash
wardian browser browser:1 downloads
```

The footer of a Browser tab shows a failed-request count beside the console
error count, so a page that is quietly erroring says so without anyone asking.

::: warning
`network <id>` prints `Authorization` and `Cookie` headers, and `cookies` prints
cookie values — that is the point of the commands. These are the session's own
credentials rather than yours, but the output is still secret material. Keep it
out of pull requests, artifacts, and anywhere else it outlives the task.
:::

## Session Lifetime

A browser session belongs to the app, not to the tab presenting it. Closing the
tab detaches the view and leaves the page running.

A session ends when you run `wardian browser close`, when the agent that owns
it stops, or when Wardian exits.

## After a Restart

Browser tabs come back with the app. A restored tab reopens its page by itself
the first time you look at it, at the same address and the same size as before.
Tabs you do not switch to stay dormant, so a restart never starts a browser you
were not about to use.

The page comes back; the session does not. Each session gets a fresh isolated
profile, so cookies, logins, and web storage from before the restart are gone.

Two cases stay manual on purpose:

- **A browser that crashed while you were watching it** shows **Browser session
  unavailable** and waits, so a crash is something you see rather than
  something that quietly restarts.
- **A tab that was left blank** has no address worth reopening.

Both show a **Reopen this page** button. If reopening fails — usually no
Chromium installed — the tab says why instead of retrying in the background.

## Requirements

The Browser surface needs a Chromium-based browser installed. Wardian looks for
Microsoft Edge, Google Chrome, Brave, then Chromium, and uses the first one it
finds. Set `WARDIAN_BROWSER_BINARY` to an absolute path to choose a specific
one.

If none is found, opening a Browser surface reports that no engine is available
and names the override rather than showing a blank page.
