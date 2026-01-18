# Text Editor Recipes

These recipes build essential text editors for system administration
and general editing tasks.

## Package List

| Package | Version | Description |
|---------|---------|-------------|
| vim | 9.2 | Vi Improved - powerful modal text editor |
| nano | 8.0 | GNU nano - simple, user-friendly editor |

## Build Order

```
ncurses (from libs/)
    │
    ├────────────┐
    ▼            ▼
┌───────┐   ┌────────┐
│  vim  │   │  nano  │  (can build in parallel)
└───────┘   └────────┘
```

## Building Editors

```bash
# Build all editors (orchestrated)
conary bootstrap editors

# Or build individual editors
conary cook recipes/core/editors/vim.toml
conary cook recipes/core/editors/nano.toml
```

## Vim

Vim (Vi Improved) is a highly configurable, modal text editor. It's the
standard editor for UNIX system administration and software development.

### Key Features

- **Modal editing**: Normal, Insert, Visual, Command modes
- **Extensible**: Thousands of plugins available
- **Powerful**: Macros, regex search/replace, split windows
- **Universal**: Available on virtually every UNIX system

### Installed Commands

| Command | Description |
|---------|-------------|
| vim | Vi Improved editor |
| vi | Symlink to vim |
| view | Read-only vim |
| vimdiff | Diff mode vim |
| ex | Ex mode vim |

### Basic Vim Usage

```
NORMAL MODE (default - for navigation and commands)
  h j k l     Move cursor (left, down, up, right)
  w b         Move by word (forward, backward)
  0 $         Move to line start/end
  gg G        Move to file start/end
  /pattern    Search forward
  n N         Next/previous search result
  dd          Delete line
  yy          Yank (copy) line
  p P         Paste after/before cursor
  u           Undo
  Ctrl+r      Redo

INSERT MODE (for typing text)
  i           Insert before cursor
  a           Insert after cursor
  o           Open new line below
  O           Open new line above
  Esc         Return to normal mode

COMMAND MODE (for saving, quitting, etc.)
  :w          Save file
  :q          Quit
  :wq         Save and quit
  :q!         Quit without saving
  :e file     Edit another file
  :%s/a/b/g   Replace all 'a' with 'b'
```

### Vim Configuration

System-wide configuration: `/etc/vimrc`
User configuration: `~/.vimrc`

**Example ~/.vimrc additions:**
```vim
" Show line numbers
set number

" Enable relative line numbers
set relativenumber

" Highlight current line
set cursorline

" Enable mouse support
set mouse=a

" Use system clipboard
set clipboard=unnamedplus
```

## Nano

GNU nano is a simple, modeless text editor. It's ideal for users who
prefer straightforward editing without learning modal commands.

### Key Features

- **Modeless**: No modes to switch between
- **Intuitive**: On-screen shortcut hints
- **Simple**: Easy to learn and use
- **Capable**: Syntax highlighting, search/replace, undo

### Installed Commands

| Command | Description |
|---------|-------------|
| nano | GNU nano editor |
| rnano | Restricted nano (limited features) |

### Basic Nano Usage

Nano displays shortcuts at the bottom of the screen.
`^` means Ctrl, `M-` means Alt.

```
NAVIGATION
  Arrow keys    Move cursor
  Ctrl+A        Beginning of line
  Ctrl+E        End of line
  Ctrl+Y        Page up
  Ctrl+V        Page down
  Ctrl+_        Go to line number

EDITING
  Ctrl+K        Cut line
  Ctrl+U        Paste
  Alt+6         Copy line
  Ctrl+\        Search and replace

FILE OPERATIONS
  Ctrl+O        Save file
  Ctrl+X        Exit
  Ctrl+R        Read file into current buffer

SEARCH
  Ctrl+W        Search
  Alt+W         Repeat search
```

### Nano Configuration

System-wide configuration: `/etc/nanorc`
User configuration: `~/.nanorc`

**Example ~/.nanorc additions:**
```
## Show line numbers
set linenumbers

## Enable mouse support
set mouse

## Don't wrap long lines
set nowrap

## Use spaces instead of tabs
set tabstospaces
set tabsize 4
```

## Editor Comparison

| Feature | Vim | Nano |
|---------|-----|------|
| Learning curve | Steep | Gentle |
| Editing model | Modal | Modeless |
| Customization | Extensive | Basic |
| Plugin ecosystem | Huge | Limited |
| Remote editing | Excellent | Good |
| Startup time | Fast | Instant |
| Memory usage | Low | Very low |

**Choose Vim if:**
- You edit text frequently
- You want maximum efficiency once learned
- You need advanced features (macros, regex, splits)

**Choose Nano if:**
- You make occasional quick edits
- You prefer simplicity
- You don't want to learn modal editing

## Setting Default Editor

Most programs respect the `EDITOR` and `VISUAL` environment variables:

```bash
# In ~/.bashrc or /etc/profile
export EDITOR=vim
export VISUAL=vim

# Or for nano users
export EDITOR=nano
export VISUAL=nano
```

Programs that use these variables:
- git (commit messages)
- crontab -e
- visudo
- systemctl edit

## Emergency Editing

Both vim and nano are installed in `/bin` for emergency access when
`/usr` may not be mounted. This is critical for system recovery.

```bash
# Even with minimal PATH
/bin/vi /etc/fstab
/bin/nano /etc/fstab
```

## Verification

```bash
# Check vim
vim --version | head -5

# Check nano
nano --version

# Test editing
echo "test" > /tmp/test.txt
vim /tmp/test.txt
nano /tmp/test.txt
rm /tmp/test.txt
```

## Next Steps

After building editors, consider:
- Boot tools (grub)
- Additional utilities
