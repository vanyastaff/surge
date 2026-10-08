# R19 managed MCP backend environment discovery

Read-only; no source edits, Cargo builds, installation, image download, privilege requests, remote host connections or environment/secret inventory.

## Actual local environment

Commands `sw_vers`, `sysctl kern.hv_support hw.memsize hw.ncpu`, `df -h .`:
- macOS27.0 build26A428, Apple ARM native toolchain.
- kern.hv_support=1, total RAM8GiB,8CPUs.
- Available disk6.1GiB at inspection (less than parent estimated8GiB).
- `/usr/bin/swift` and `/usr/bin/xcrun` exist; SDK `/Library/Developer/CommandLineTools/SDKs/MacOSX.sdk` includes Virtualization.framework headers.
- No tart,qemu-system-aarch64,qemu-system-x86_64,prlctl,VBoxManage,container,docker,podman,limactl on PATH or normal /opt/homebrew/bin,/usr/local/bin,/usr/bin locations.
- No matching VM/container app in /Applications or ~/Applications (Parallels,UTM,VirtualBox,Docker,OrbStack,VMware,Tart names).
- Known runtime state paths ~/.tart,~/.lima,~/.local/share/containers,~/Library/Containers/com.utmapp.UTM,~/Library/Application Support/UTM,~/Library/Application Support/com.apple.container absent.
- No guest disk/kernel/initrd files in project outside ignored target.

## Supported platform API prerequisite

Installed SDK VZVirtualMachine.h says constructing VMs requires `com.apple.security.virtualization` entitlement. VZLinuxBootLoader.h requires a local Linux kernelURL and optionally initrd. Hardware support+SDK presence is not a runnable/signed managed backend and does not prove machine launch/stop/identity/poweroff fencing.

Primary current Apple container README https://github.com/apple/container describes Linux containers in lightweight VMs, optimized for Apple silicon and supported on macOS26. Installer requires admin password for /usr/local. Local macOS27 hardware is suitable in principle, but no installed container runtime/image/service and no macOS27 smoke was performed. No source builds/downloads/install actions undertaken. This backend executes a Linux guest, not native Mac commands; manifest/input ABI must name Linux explicitly and cannot be silently rewritten from a Mac execution contract.

## Existing project runner evidence

Read .github/workflows/*.yml, .cargo/config.toml, .ai-factory/config.yaml, surge.toml/surge.example.toml and narrow runner configuration searches.
- Existing CI uses GitHub-hosted ubuntu/windows/macOS labels only.
- No self-hosted runner, local Linux/Windows endpoint, explicit SSH host or managedVM/container runtime setting found.
- .cargo/config.toml only local package debug-info override; no remote execution.
- No remote connection or GitHub workflow execution attempted; a runner label is not an available authorized interactive local execution endpoint.

## One viable prototype direction and exact blocker

For a Linux-declared managed MCP capability: Apple `container` is a viable supported implementation substrate to prototype once its signed runtime+service and a pinned verified Linux MCP image are provisioned. Needs guest-side owner supervisor, per-run VM identity, bounded workspace ingress/egress, explicit effects policy and poweroff/stop proof; a successful process exit alone must not close descendants/external effects. Available6.1GiB disk must be budgeted before image/runtime setup. None of those primitives are currently runnable here.

For an unchanged native-Mac execution manifest: Apple container cannot fulfill it. A macOS guest on Virtualization.framework would preserve native platform semantics, but requires signed entitled helper, verified compatible macOS restore/installed guest image, explicit storage/RAM budget, guest ownership supervisor and tested VM settlement. Those prerequisites are absent; SDK availability alone cannot substantiate completion.

Verdict: no installed managed backend or configured reachable native runner to implement and fully verify today within read-only constraints. Exact external blocker is provisioning a supported runtime+guest/image with the manifest's declared platform, plus sufficient disk and signing/entitlement prerequisites. No no-descendant shortcut, fake coverage or cross-platform rewrite proposed.
