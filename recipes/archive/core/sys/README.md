# System Utilities Recipes

These recipes build essential system administration utilities for
process management, user administration, and privilege escalation.

## Package List

| Package | Version | Description |
|---------|---------|-------------|
| procps-ng | 4.1.0 | Process monitoring utilities (ps, top, free, vmstat) |
| psmisc | 24.0 | Miscellaneous process utilities (fuser, killall, pstree) |
| shadow | 4.16.0 | User and group management (useradd, passwd, login) |
| sudo | 1.9.18 | Privilege escalation (run commands as another user) |

## Build Order

```
linux-pam, libcap, ncurses (from libs/)
              │
    ┌─────────┼─────────┐
    ▼         ▼         ▼
┌────────┐ ┌────────┐ ┌────────┐
│procps  │ │psmisc  │ │shadow  │  (can build in parallel)
└────────┘ └────────┘ └────────┘
                          │
                          ▼
                     ┌────────┐
                     │ sudo   │  (needs PAM from shadow setup)
                     └────────┘
```

## Building System Utilities

```bash
# Build all system utilities (orchestrated)
conary bootstrap sys

# Or build individual packages
conary cook recipes/core/sys/procps-ng.toml
conary cook recipes/core/sys/psmisc.toml
conary cook recipes/core/sys/shadow.toml
conary cook recipes/core/sys/sudo.toml
```

## Package Details

### procps-ng

Process monitoring and system information tools.

**Installed commands:**
| Command | Description |
|---------|-------------|
| ps | Report process status |
| top | Dynamic process viewer |
| free | Display memory usage |
| vmstat | Virtual memory statistics |
| pgrep | Find processes by name |
| pkill | Kill processes by name |
| pidof | Find process ID by name |
| pmap | Report memory map of process |
| sysctl | Configure kernel parameters |
| uptime | System uptime and load |
| w | Show who is logged in |
| watch | Execute command periodically |

**Example usage:**
```bash
# List all processes
ps aux

# Interactive process viewer
top

# Memory usage
free -h

# Find process by name
pgrep -l nginx

# Kill by name
pkill -9 hung_process

# System statistics
vmstat 1 5
```

### psmisc

Additional process management utilities.

**Installed commands:**
| Command | Description |
|---------|-------------|
| fuser | Identify processes using files |
| killall | Kill processes by name |
| pstree | Display process tree |
| peekfd | Peek at file descriptors |

**Example usage:**
```bash
# Show processes using a file
fuser -v /var/log/syslog

# Kill all instances of a program
killall firefox

# Display process tree
pstree -p

# Show who's using a mount point
fuser -vm /mnt/usb
```

### shadow

User and group management with shadow password support.

**Installed commands:**
| Command | Description |
|---------|-------------|
| useradd | Create new user |
| usermod | Modify user account |
| userdel | Delete user account |
| groupadd | Create new group |
| groupmod | Modify group |
| groupdel | Delete group |
| passwd | Change password |
| chage | Change password aging |
| login | Begin session |
| su | Switch user |
| chpasswd | Batch update passwords |
| newusers | Batch create users |

**Example usage:**
```bash
# Create new user with home directory
useradd -m -s /bin/bash newuser

# Add user to supplementary group
usermod -aG wheel newuser

# Set password
passwd newuser

# Create system user (no login)
useradd -r -s /sbin/nologin serviceuser

# Delete user and home directory
userdel -r olduser
```

### sudo

Privilege escalation for controlled root access.

**Installed commands:**
| Command | Description |
|---------|-------------|
| sudo | Execute command as another user |
| sudoedit | Edit files as another user |
| visudo | Safely edit sudoers file |
| sudoreplay | Replay sudo session logs |

**Example usage:**
```bash
# Run command as root
sudo systemctl restart nginx

# Run command as specific user
sudo -u postgres psql

# Edit file as root
sudoedit /etc/hosts

# List allowed commands
sudo -l

# Run shell as root
sudo -i
```

## Configuration

### /etc/login.defs

Shadow's main configuration for password policies:
```
PASS_MAX_DAYS   99999    # Maximum password age
PASS_MIN_DAYS   0        # Minimum days between changes
PASS_WARN_AGE   7        # Warning days before expiration
UID_MIN         1000     # Minimum UID for regular users
ENCRYPT_METHOD  YESCRYPT # Password hashing algorithm
```

### /etc/sudoers

Sudo configuration (always edit with `visudo`):
```
# Allow root full access
root    ALL=(ALL:ALL) ALL

# Allow wheel group members full access
%wheel  ALL=(ALL:ALL) ALL
```

### /etc/sudoers.d/

Drop-in directory for additional sudo rules:
```bash
# Create rule file
visudo -f /etc/sudoers.d/developers

# Example: Allow developers to restart services
%developers ALL=(root) /usr/bin/systemctl restart *
```

## PAM Integration

Both shadow and sudo integrate with PAM (Pluggable Authentication Modules).

**Installed PAM configurations:**
- `/etc/pam.d/login` - Console login
- `/etc/pam.d/passwd` - Password changes
- `/etc/pam.d/su` - User switching
- `/etc/pam.d/sudo` - Sudo authentication

These reference `system-auth` which should be configured in linux-pam.

## Security Considerations

### Shadow Passwords

Shadow keeps password hashes in `/etc/shadow` (root-only readable)
instead of world-readable `/etc/passwd`.

### Sudo Best Practices

1. **Use wheel group**: Add trusted users to wheel instead of editing sudoers
2. **Require passwords**: Avoid NOPASSWD unless necessary
3. **Principle of least privilege**: Grant only needed commands
4. **Log everything**: Default config logs all sudo activity
5. **Use visudo**: Never edit sudoers directly

### Password Hashing

Shadow uses YESCRYPT by default (modern, memory-hard algorithm).
More secure than older SHA-512 for password storage.

## Verification

```bash
# Check procps
ps --version
free -h

# Check psmisc
pstree --version
killall --version

# Check shadow
useradd --version
passwd --version

# Check sudo
sudo --version
sudo -l
```

## Troubleshooting

### "sudo: no tty present"

When running sudo in scripts without a terminal:
```bash
# Use -n for non-interactive (fails if password needed)
sudo -n command

# Or allocate pseudo-tty
ssh -t user@host 'sudo command'
```

### "Authentication failure"

1. Verify user is in wheel group: `groups username`
2. Check PAM configuration: `ls -la /etc/pam.d/sudo`
3. Verify system-auth exists: `ls -la /etc/pam.d/system-auth`

### User cannot login

1. Check shell is valid: `grep username /etc/passwd`
2. Verify shell exists: `ls -la /bin/bash`
3. Check account status: `passwd -S username`

## Next Steps

After building system utilities, consider:
- Editors (vim, nano)
- Boot tools (grub)
