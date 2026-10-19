<div class="rooz-hero">
<div class="rooz-hero-text">
<p class="rooz-eyebrow">Open source CLI · Linux &amp; macOS</p>
<h1 class="rooz-title">Work in <span>containers</span>.</h1>
<p class="rooz-lede">Rooz runs your dev workspaces in containers, on a local or remote Docker/Podman engine. Git cloning, SSH keys, encrypted secrets, shared caches, sidecars and declarative config are built in.</p>
<div class="rooz-install"><span class="rooz-prompt">$</span><code>curl -fsSL https://rooz.dev/install.sh | sh</code><button class="rooz-copy" type="button" data-copy="curl -fsSL https://rooz.dev/install.sh | sh" title="Copy">Copy</button></div>
<div class="rooz-cta"><a class="rooz-btn rooz-btn-primary" href="getting-started.html">Get started <svg class="rooz-arrow" viewBox="0 0 16 16" aria-hidden="true"><path d="M3 8h10M9 4l4 4-4 4"/></svg></a><a class="rooz-btn" href="https://github.com/queil/rooz">GitHub</a></div>
</div>
<div class="rooz-hero-art"><img src="rooz512.webp" alt="Kangaroos carrying a shipping container"></div>
</div>

<div class="rooz-terminal" aria-label="Quick start">
<div class="rooz-terminal-bar"><i></i><i></i><i></i><span>quick start</span></div>
<pre><span class="t-p">$</span> rooz system init                                   <span class="t-c"># SSH and age keys</span>
<span class="t-p">$</span> rooz new -g git@github.com:your/repo myworkspace   <span class="t-c"># create a workspace from a repo</span>
<span class="t-p">$</span> rooz enter myworkspace                             <span class="t-c"># open a shell in it</span>
<span class="t-p">$</span> rooz code myworkspace                              <span class="t-c"># or open it in VS Code</span>
<span class="t-p">$</span> <span class="t-cursor"></span></pre>
</div>

<div class="rooz-features">
<a class="rooz-card" href="usage.html#create-a-workspace"><b>Git built in</b><span>Clone a repo straight into the workspace volume - plus extra repos alongside it.</span></a>
<a class="rooz-card" href="getting-started.html#initialize"><b>SSH keys</b><span>One generated ed25519 key, mounted read-only into every work container.</span></a>
<a class="rooz-card" href="secrets.html"><b>Git-safe secrets</b><span>age-encrypted values in config. The identity never leaves your machine.</span></a>
<a class="rooz-card" href="usage.html#manage-caches"><b>Shared caches</b><span>Package caches shared across workspaces, so builds stay warm.</span></a>
<a class="rooz-card" href="sidecars.html"><b>Sidecars</b><span>Databases and services next to your workspace - a minimal compose.</span></a>
<a class="rooz-card" href="remote-hosts.html"><b>Remote hosts</b><span>Shares as little as possible with the host, so workspaces run on remote engines too.</span></a>
</div>

<a class="rooz-band" href="vscode.html"><b>VS Code, inside the container</b><span><code>rooz code myworkspace</code> attaches VS Code to the workspace. Extensions, terminals and debuggers run in the container; a volume keeps them across rebuilds.</span></a>

## Requirements

One of:
* Docker >= 29.5.0
* Podman >= 6
* Rancher Desktop - untested

The engine must be yours: a rootless daemon on your machine, or a remote one only you use. Rooz protects your host, other workspaces and secrets from the repositories you open, but not from other users of the same engine - anyone with engine API access can read every volume, run containers on your networks and spoof image/volume names. Don't share an engine with untrusted users.
