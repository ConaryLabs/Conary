# Version Control Recipes

These recipes build version control tools for source code management.

## Package List

| Package | Version | Description |
|---------|---------|-------------|
| git | 2.53.0 | Fast distributed version control system |

## Build Order

```
OpenSSL, curl, zlib (from libs/ and net/)
         │
         ▼
    ┌─────────┐
    │   git   │
    └─────────┘
```

## Building Version Control Tools

```bash
# Build git (orchestrated)
conary bootstrap vcs

# Or build directly
conary cook recipes/core/vcs/git.toml
```

## Git Overview

Git is the de facto standard for version control, used by:
- The Linux kernel project (where Git originated)
- GitHub, GitLab, Bitbucket
- Nearly all modern software projects

### Key Features

- **Distributed**: Every clone is a full repository
- **Fast**: Local operations are nearly instant
- **Branching**: Lightweight branching and merging
- **Data integrity**: SHA-1 hashing ensures data integrity
- **Staging area**: Fine-grained control over commits

## Configuration

### System Configuration

The recipe installs a sensible `/etc/gitconfig`:

```ini
[init]
    defaultBranch = main

[core]
    autocrlf = input

[color]
    ui = auto

[pull]
    rebase = false

[http]
    sslCAPath = /etc/ssl/certs
    sslCAInfo = /etc/ssl/certs/ca-certificates.crt
```

### User Configuration

Users should set up their identity:

```bash
git config --global user.name "Your Name"
git config --global user.email "your.email@example.com"
```

## Common Operations

### Repository Setup

```bash
# Initialize new repository
git init

# Clone existing repository
git clone https://github.com/user/repo.git

# Clone with specific branch
git clone -b develop https://github.com/user/repo.git
```

### Basic Workflow

```bash
# Check status
git status

# Stage changes
git add file.txt
git add -A              # Add all changes

# Commit
git commit -m "Add feature"

# Push to remote
git push origin main

# Pull updates
git pull
```

### Branching

```bash
# Create and switch to branch
git checkout -b feature-branch

# List branches
git branch -a

# Switch branches
git checkout main

# Merge branch
git merge feature-branch

# Delete branch
git branch -d feature-branch
```

### History

```bash
# View log
git log
git log --oneline --graph

# Show specific commit
git show abc1234

# Show changes
git diff
git diff --staged
```

### Remote Operations

```bash
# Add remote
git remote add origin https://github.com/user/repo.git

# List remotes
git remote -v

# Fetch updates
git fetch origin

# Push new branch
git push -u origin feature-branch
```

## Shell Integration

The recipe installs shell completion and prompt integration:

### Bash Completion

Bash completion is installed automatically to:
```
/usr/share/bash-completion/completions/git
```

### Git Prompt

Add to your `~/.bashrc` for branch info in prompt:

```bash
source /usr/share/git/git-prompt.sh
PS1='[\u@\h \W$(__git_ps1 " (%s)")]\$ '
```

## Build Options

The recipe builds Git with these options:

| Option | Setting | Reason |
|--------|---------|--------|
| `USE_LIBPCRE2=` | Disabled | Not required for basic functionality |
| `NO_EXPAT=1` | Enabled | Disables git-http-push (rarely needed) |
| `NO_TCLTK=1` | Enabled | No graphical tools (gitk, git-gui) |
| `NO_GETTEXT=1` | Enabled | English only (smaller install) |

For a full-featured build with GUI tools and internationalization,
these options can be removed in a later stage.

## Verification

```bash
# Check version
git --version

# Test clone over HTTPS
git clone --depth 1 https://github.com/git/git.git /tmp/git-test
rm -rf /tmp/git-test

# Check configuration
git config --list --show-origin
```

## Troubleshooting

### SSL Certificate Errors

If you get certificate errors:

```bash
# Verify CA certificates are installed
ls -la /etc/ssl/certs/ca-certificates.crt

# Check git's SSL configuration
git config --global http.sslCAInfo /etc/ssl/certs/ca-certificates.crt
```

### Permission Denied (SSH)

For SSH-based remotes:

```bash
# Check SSH key
ssh -T git@github.com

# Add SSH key to agent
eval "$(ssh-agent -s)"
ssh-add ~/.ssh/id_ed25519
```

### Large Repositories

For large repositories:

```bash
# Shallow clone
git clone --depth 1 https://github.com/user/large-repo.git

# Partial clone (blobs on demand)
git clone --filter=blob:none https://github.com/user/large-repo.git
```

## Future Additions

Other version control systems that could be added later:
- **mercurial**: Python-based DVCS
- **subversion**: Centralized VCS (for legacy projects)
- **fossil**: Self-contained VCS with wiki/tickets

For bootstrap purposes, Git alone is sufficient.

## Next Steps

After building version control tools, consider:
- System utilities (procps, psmisc, shadow, sudo)
- Editors (vim, nano)
- Boot tools (grub)
