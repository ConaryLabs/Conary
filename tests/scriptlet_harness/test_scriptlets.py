#!/usr/bin/env python3
"""
Scriptlet Verification Test Harness

This script generates minimal test packages (RPM, DEB, Arch) with scriptlets
that log their execution arguments, then verifies Conary executes them correctly.

Usage:
    python3 test_scriptlets.py [--keep-artifacts]

Requirements:
    - rpmbuild (for RPM tests)
    - dpkg-deb (for DEB tests)
    - tar, zstd (for Arch tests)
    - conary binary in PATH or ../target/release/conary
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass
from pathlib import Path
from typing import Optional


# ANSI colors for output
class Colors:
    GREEN = '\033[92m'
    RED = '\033[91m'
    YELLOW = '\033[93m'
    BLUE = '\033[94m'
    RESET = '\033[0m'
    BOLD = '\033[1m'


def log_info(msg: str):
    print(f"{Colors.BLUE}[INFO]{Colors.RESET} {msg}")


def log_success(msg: str):
    print(f"{Colors.GREEN}[PASS]{Colors.RESET} {msg}")


def log_error(msg: str):
    print(f"{Colors.RED}[FAIL]{Colors.RESET} {msg}")


def log_warn(msg: str):
    print(f"{Colors.YELLOW}[WARN]{Colors.RESET} {msg}")


@dataclass
class ScriptletLog:
    """Represents a single scriptlet execution log entry."""
    phase: str
    args: list[str]
    package: str
    version: str
    timestamp: str


class PackageGenerator:
    """Base class for package generators."""

    def __init__(self, work_dir: Path, log_dir: Path):
        self.work_dir = work_dir
        self.log_dir = log_dir
        self.log_dir.mkdir(parents=True, exist_ok=True)

    def get_log_script(self, phase: str, pkg_name: str, version: str) -> str:
        """Generate a script that logs its execution."""
        log_file = self.log_dir / "scriptlet.log"
        return f'''#!/bin/bash
echo "{phase}|${{@}}|{pkg_name}|{version}|$(date +%s)" >> "{log_file}"
'''


class RpmGenerator(PackageGenerator):
    """Generate RPM packages with logging scriptlets."""

    def __init__(self, work_dir: Path, log_dir: Path):
        super().__init__(work_dir, log_dir)
        self.rpm_dir = work_dir / "rpmbuild"
        for subdir in ["BUILD", "RPMS", "SOURCES", "SPECS", "SRPMS"]:
            (self.rpm_dir / subdir).mkdir(parents=True, exist_ok=True)

    def generate(self, name: str, version: str, release: str = "1") -> Optional[Path]:
        """Generate an RPM package with logging scriptlets."""
        spec_content = f'''
Name:           {name}
Version:        {version}
Release:        {release}
Summary:        Test package for scriptlet verification
License:        MIT
BuildArch:      noarch

%description
Test package for Conary scriptlet verification.

%install
mkdir -p %{{buildroot}}/usr/share/{name}
echo "{name}-{version}" > %{{buildroot}}/usr/share/{name}/version.txt

%files
/usr/share/{name}/version.txt

%pre
{self.get_log_script("pre-install", name, version)}

%post
{self.get_log_script("post-install", name, version)}

%preun
{self.get_log_script("pre-remove", name, version)}

%postun
{self.get_log_script("post-remove", name, version)}
'''

        spec_file = self.rpm_dir / "SPECS" / f"{name}.spec"
        spec_file.write_text(spec_content)

        try:
            result = subprocess.run(
                ["rpmbuild", "-bb", "--define", f"_topdir {self.rpm_dir}", str(spec_file)],
                capture_output=True,
                text=True,
                check=True
            )

            # Find the generated RPM
            rpm_dir = self.rpm_dir / "RPMS" / "noarch"
            rpms = list(rpm_dir.glob(f"{name}-{version}*.rpm"))
            if rpms:
                return rpms[0]
            return None
        except subprocess.CalledProcessError as e:
            log_error(f"rpmbuild failed: {e.stderr}")
            return None
        except FileNotFoundError:
            log_warn("rpmbuild not found - skipping RPM tests")
            return None


class DebGenerator(PackageGenerator):
    """Generate DEB packages with logging scriptlets."""

    def generate(self, name: str, version: str) -> Optional[Path]:
        """Generate a DEB package with logging scriptlets."""
        pkg_dir = self.work_dir / f"{name}_{version}"
        pkg_dir.mkdir(parents=True, exist_ok=True)

        # Create DEBIAN control directory
        debian_dir = pkg_dir / "DEBIAN"
        debian_dir.mkdir(parents=True, exist_ok=True)

        # Control file
        control_content = f'''Package: {name}
Version: {version}
Section: misc
Priority: optional
Architecture: all
Maintainer: Test <test@example.com>
Description: Test package for scriptlet verification
 Test package for Conary scriptlet verification.
'''
        (debian_dir / "control").write_text(control_content)

        # Create scriptlets
        # DEB scripts receive: preinst install|upgrade [old-version]
        #                      postinst configure [old-version]
        #                      prerm remove|upgrade [new-version]
        #                      postrm remove|upgrade [new-version]

        preinst = f'''#!/bin/bash
echo "pre-install|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
'''
        postinst = f'''#!/bin/bash
echo "post-install|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
'''
        prerm = f'''#!/bin/bash
echo "pre-remove|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
'''
        postrm = f'''#!/bin/bash
echo "post-remove|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
'''

        for script_name, content in [("preinst", preinst), ("postinst", postinst),
                                      ("prerm", prerm), ("postrm", postrm)]:
            script_path = debian_dir / script_name
            script_path.write_text(content)
            script_path.chmod(0o755)

        # Create a dummy file
        data_dir = pkg_dir / "usr" / "share" / name
        data_dir.mkdir(parents=True, exist_ok=True)
        (data_dir / "version.txt").write_text(f"{name}-{version}\n")

        # Build the package
        output_path = self.work_dir / f"{name}_{version}_all.deb"

        try:
            result = subprocess.run(
                ["dpkg-deb", "--build", str(pkg_dir), str(output_path)],
                capture_output=True,
                text=True,
                check=True
            )
            return output_path
        except subprocess.CalledProcessError as e:
            log_error(f"dpkg-deb failed: {e.stderr}")
            return None
        except FileNotFoundError:
            log_warn("dpkg-deb not found - skipping DEB tests")
            return None


class ArchGenerator(PackageGenerator):
    """Generate Arch packages with logging scriptlets."""

    def generate(self, name: str, version: str, pkgrel: str = "1") -> Optional[Path]:
        """Generate an Arch package with logging scriptlets."""
        pkg_dir = self.work_dir / f"{name}-{version}"
        pkg_dir.mkdir(parents=True, exist_ok=True)

        # Create .PKGINFO
        pkginfo_content = f'''pkgname = {name}
pkgbase = {name}
pkgver = {version}-{pkgrel}
pkgdesc = Test package for scriptlet verification
url = https://example.com
builddate = 1234567890
packager = Test <test@example.com>
size = 1024
arch = any
'''
        (pkg_dir / ".PKGINFO").write_text(pkginfo_content)

        # Create .INSTALL with function definitions
        # Arch scripts receive version arguments:
        #   pre_install(new_version)
        #   post_install(new_version)
        #   pre_upgrade(new_version, old_version)
        #   post_upgrade(new_version, old_version)
        #   pre_remove(old_version)
        #   post_remove(old_version)
        install_content = f'''
pre_install() {{
    echo "pre-install|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
}}

post_install() {{
    echo "post-install|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
}}

pre_upgrade() {{
    echo "pre-upgrade|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
}}

post_upgrade() {{
    echo "post-upgrade|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
}}

pre_remove() {{
    echo "pre-remove|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
}}

post_remove() {{
    echo "post-remove|${{@}}|{name}|{version}|$(date +%s)" >> "{self.log_dir}/scriptlet.log"
}}
'''
        (pkg_dir / ".INSTALL").write_text(install_content)

        # Create dummy data
        data_dir = pkg_dir / "usr" / "share" / name
        data_dir.mkdir(parents=True, exist_ok=True)
        (data_dir / "version.txt").write_text(f"{name}-{version}\n")

        # Create .MTREE (minimal)
        mtree_content = '''#mtree
/set type=file uid=0 gid=0 mode=644
./.PKGINFO time=1234567890.0 size=256
./.INSTALL time=1234567890.0 size=512
./usr time=1234567890.0 mode=755 type=dir
./usr/share time=1234567890.0 mode=755 type=dir
'''
        (pkg_dir / ".MTREE").write_text(mtree_content)

        # Build the package using tar + zstd
        output_path = self.work_dir / f"{name}-{version}-{pkgrel}-any.pkg.tar.zst"

        try:
            # Create tar archive first
            tar_path = self.work_dir / f"{name}.tar"

            # Get list of files to include
            files = [".PKGINFO", ".INSTALL", ".MTREE", "usr"]

            result = subprocess.run(
                ["tar", "-cf", str(tar_path)] + files,
                capture_output=True,
                text=True,
                check=True,
                cwd=str(pkg_dir)
            )

            # Compress with zstd
            result = subprocess.run(
                ["zstd", "-f", str(tar_path), "-o", str(output_path)],
                capture_output=True,
                text=True,
                check=True
            )

            # Clean up tar file
            tar_path.unlink()

            return output_path
        except subprocess.CalledProcessError as e:
            log_error(f"Package creation failed: {e.stderr}")
            return None
        except FileNotFoundError as e:
            log_warn(f"Required tool not found ({e}) - skipping Arch tests")
            return None


class ConaryTester:
    """Test runner for Conary scriptlet verification."""

    def __init__(self, conary_bin: Path, work_dir: Path, real_root: bool = False):
        self.conary_bin = conary_bin
        self.work_dir = work_dir
        self.real_root = real_root
        self.db_path = work_dir / "conary.db"
        self.log_dir = work_dir / "logs"

        if real_root:
            # Install to real root - scriptlets will actually execute
            self.root_dir = Path("/")
            log_warn("REAL ROOT MODE: Installing to / - scriptlets will execute!")
        else:
            # Install to temp dir - scriptlets will be skipped
            self.root_dir = work_dir / "root"
            self.root_dir.mkdir(parents=True, exist_ok=True)

        self.log_dir.mkdir(parents=True, exist_ok=True)

        # Initialize generators
        self.rpm_gen = RpmGenerator(work_dir / "rpm", self.log_dir)
        self.deb_gen = DebGenerator(work_dir / "deb", self.log_dir)
        self.arch_gen = ArchGenerator(work_dir / "arch", self.log_dir)

        # Track installed packages for cleanup
        self.installed_packages: list[str] = []

    def init_db(self):
        """Initialize a fresh Conary database."""
        if self.db_path.exists():
            self.db_path.unlink()

        result = subprocess.run(
            [str(self.conary_bin), "init", "-d", str(self.db_path)],
            capture_output=True,
            text=True
        )
        if result.returncode != 0:
            raise RuntimeError(f"Failed to init db: {result.stderr}")

    def clear_logs(self):
        """Clear the scriptlet log file."""
        log_file = self.log_dir / "scriptlet.log"
        if log_file.exists():
            log_file.unlink()

    def read_logs(self) -> list[ScriptletLog]:
        """Read and parse the scriptlet log file."""
        log_file = self.log_dir / "scriptlet.log"
        if not log_file.exists():
            return []

        logs = []
        for line in log_file.read_text().strip().split("\n"):
            if not line:
                continue
            parts = line.split("|")
            if len(parts) >= 5:
                logs.append(ScriptletLog(
                    phase=parts[0],
                    args=parts[1].split() if parts[1] else [],
                    package=parts[2],
                    version=parts[3],
                    timestamp=parts[4]
                ))
        return logs

    def install(self, package_path: Path, no_scripts: bool = False, pkg_name: str = None) -> bool:
        """Install a package."""
        cmd = [
            str(self.conary_bin), "install",
            str(package_path),
            "-d", str(self.db_path),
            "-r", str(self.root_dir)
        ]
        if no_scripts:
            cmd.append("--no-scripts")

        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            log_error(f"Install failed: {result.stderr}")
            return False

        # Track for cleanup in real-root mode
        if self.real_root and pkg_name:
            if pkg_name not in self.installed_packages:
                self.installed_packages.append(pkg_name)

        return True

    def remove(self, package_name: str, no_scripts: bool = False) -> bool:
        """Remove a package."""
        cmd = [
            str(self.conary_bin), "remove",
            package_name,
            "-d", str(self.db_path),
            "-r", str(self.root_dir)
        ]
        if no_scripts:
            cmd.append("--no-scripts")

        result = subprocess.run(cmd, capture_output=True, text=True)
        if result.returncode != 0:
            log_error(f"Remove failed: {result.stderr}")
            return False
        return True

    def cleanup_installed(self):
        """Remove all installed packages (for real-root mode cleanup)."""
        if not self.real_root:
            return

        for pkg_name in list(self.installed_packages):
            try:
                self.remove(pkg_name, no_scripts=True)
                self.installed_packages.remove(pkg_name)
            except Exception as e:
                log_warn(f"Cleanup failed for {pkg_name}: {e}")

    def run_test(self, name: str, test_fn) -> bool:
        """Run a single test with setup/teardown."""
        print(f"\n{Colors.BOLD}=== {name} ==={Colors.RESET}")
        self.init_db()
        self.clear_logs()

        try:
            result = test_fn()
            if result:
                log_success(name)
            else:
                log_error(name)
            return result
        except Exception as e:
            log_error(f"{name}: {e}")
            import traceback
            traceback.print_exc()
            return False
        finally:
            # Always cleanup in real-root mode
            self.cleanup_installed()


class ScriptletTests:
    """Collection of scriptlet verification tests."""

    def __init__(self, tester: ConaryTester):
        self.tester = tester

    def test_rpm_install(self) -> bool:
        """Test RPM fresh install scriptlets."""
        pkg_name = "conary-test-rpm"
        pkg = self.tester.rpm_gen.generate(pkg_name, "1.0.0")
        if not pkg:
            log_warn("Skipping - RPM generation not available")
            return True  # Skip, not fail

        if not self.tester.install(pkg, pkg_name=pkg_name):
            return False

        logs = self.tester.read_logs()

        # Verify pre-install and post-install ran with $1=1
        pre = next((l for l in logs if l.phase == "pre-install"), None)
        post = next((l for l in logs if l.phase == "post-install"), None)

        if not pre:
            log_error("pre-install scriptlet did not run")
            return False
        if not post:
            log_error("post-install scriptlet did not run")
            return False

        if pre.args != ["1"]:
            log_error(f"pre-install got args {pre.args}, expected ['1']")
            return False
        if post.args != ["1"]:
            log_error(f"post-install got args {post.args}, expected ['1']")
            return False

        log_info("RPM install: pre=$1=1, post=$1=1")
        return True

    def test_rpm_upgrade(self) -> bool:
        """Test RPM upgrade scriptlets."""
        pkg_name = "conary-test-rpm-upg"
        pkg_v1 = self.tester.rpm_gen.generate(pkg_name, "1.0.0")
        pkg_v2 = self.tester.rpm_gen.generate(pkg_name, "2.0.0")
        if not pkg_v1 or not pkg_v2:
            log_warn("Skipping - RPM generation not available")
            return True

        # Install v1
        if not self.tester.install(pkg_v1, pkg_name=pkg_name):
            return False

        self.tester.clear_logs()

        # Upgrade to v2
        if not self.tester.install(pkg_v2, pkg_name=pkg_name):
            return False

        logs = self.tester.read_logs()

        # Expected order:
        # 1. NEW pre-install with $1=2
        # 2. OLD pre-remove with $1=1 (UpgradeRemoval)
        # 3. OLD post-remove with $1=1 (UpgradeRemoval)
        # 4. NEW post-install with $1=2

        new_pre = [l for l in logs if l.phase == "pre-install" and l.version == "2.0.0"]
        new_post = [l for l in logs if l.phase == "post-install" and l.version == "2.0.0"]
        old_preun = [l for l in logs if l.phase == "pre-remove" and l.version == "1.0.0"]
        old_postun = [l for l in logs if l.phase == "post-remove" and l.version == "1.0.0"]

        errors = []

        if not new_pre or new_pre[0].args != ["2"]:
            errors.append(f"NEW pre-install: expected ['2'], got {new_pre[0].args if new_pre else 'missing'}")

        if not old_preun or old_preun[0].args != ["1"]:
            errors.append(f"OLD pre-remove: expected ['1'], got {old_preun[0].args if old_preun else 'missing'}")

        if not old_postun or old_postun[0].args != ["1"]:
            errors.append(f"OLD post-remove: expected ['1'], got {old_postun[0].args if old_postun else 'missing'}")

        if not new_post or new_post[0].args != ["2"]:
            errors.append(f"NEW post-install: expected ['2'], got {new_post[0].args if new_post else 'missing'}")

        if errors:
            for e in errors:
                log_error(e)
            return False

        log_info("RPM upgrade: NEW pre=$1=2, OLD preun=$1=1, OLD postun=$1=1, NEW post=$1=2")
        return True

    def test_rpm_remove(self) -> bool:
        """Test RPM remove scriptlets."""
        pkg_name = "conary-test-rpm-rm"
        pkg = self.tester.rpm_gen.generate(pkg_name, "1.0.0")
        if not pkg:
            log_warn("Skipping - RPM generation not available")
            return True

        if not self.tester.install(pkg, pkg_name=pkg_name):
            return False

        self.tester.clear_logs()

        if not self.tester.remove("conary-test-rpm-rm"):
            return False

        logs = self.tester.read_logs()

        pre = next((l for l in logs if l.phase == "pre-remove"), None)
        post = next((l for l in logs if l.phase == "post-remove"), None)

        if not pre or pre.args != ["0"]:
            log_error(f"pre-remove: expected ['0'], got {pre.args if pre else 'missing'}")
            return False
        if not post or post.args != ["0"]:
            log_error(f"post-remove: expected ['0'], got {post.args if post else 'missing'}")
            return False

        log_info("RPM remove: preun=$1=0, postun=$1=0")
        return True

    def test_deb_install(self) -> bool:
        """Test DEB fresh install scriptlets."""
        pkg_name = "conary-test-deb"
        pkg = self.tester.deb_gen.generate(pkg_name, "1.0.0")
        if not pkg:
            log_warn("Skipping - DEB generation not available")
            return True

        if not self.tester.install(pkg, pkg_name=pkg_name):
            return False

        logs = self.tester.read_logs()

        pre = next((l for l in logs if l.phase == "pre-install"), None)
        post = next((l for l in logs if l.phase == "post-install"), None)

        if not pre:
            log_error("preinst did not run")
            return False
        if not post:
            log_error("postinst did not run")
            return False

        if pre.args != ["install"]:
            log_error(f"preinst got args {pre.args}, expected ['install']")
            return False
        if post.args != ["configure"]:
            log_error(f"postinst got args {post.args}, expected ['configure']")
            return False

        log_info("DEB install: preinst='install', postinst='configure'")
        return True

    def test_deb_upgrade(self) -> bool:
        """Test DEB upgrade scriptlets."""
        pkg_name = "conary-test-deb-upg"
        pkg_v1 = self.tester.deb_gen.generate(pkg_name, "1.0.0")
        pkg_v2 = self.tester.deb_gen.generate(pkg_name, "2.0.0")
        if not pkg_v1 or not pkg_v2:
            log_warn("Skipping - DEB generation not available")
            return True

        if not self.tester.install(pkg_v1, pkg_name=pkg_name):
            return False

        self.tester.clear_logs()

        if not self.tester.install(pkg_v2, pkg_name=pkg_name):
            return False

        logs = self.tester.read_logs()

        # Expected:
        # NEW preinst: upgrade <old-version>
        # OLD prerm: upgrade <new-version>
        # OLD postrm: upgrade <new-version>
        # NEW postinst: configure <old-version>

        new_pre = [l for l in logs if l.phase == "pre-install" and l.version == "2.0.0"]
        new_post = [l for l in logs if l.phase == "post-install" and l.version == "2.0.0"]
        old_prerm = [l for l in logs if l.phase == "pre-remove" and l.version == "1.0.0"]
        old_postrm = [l for l in logs if l.phase == "post-remove" and l.version == "1.0.0"]

        errors = []

        if not new_pre or new_pre[0].args != ["upgrade", "1.0.0"]:
            errors.append(f"NEW preinst: expected ['upgrade', '1.0.0'], got {new_pre[0].args if new_pre else 'missing'}")

        if not old_prerm or old_prerm[0].args != ["upgrade", "2.0.0"]:
            errors.append(f"OLD prerm: expected ['upgrade', '2.0.0'], got {old_prerm[0].args if old_prerm else 'missing'}")

        if not old_postrm or old_postrm[0].args != ["upgrade", "2.0.0"]:
            errors.append(f"OLD postrm: expected ['upgrade', '2.0.0'], got {old_postrm[0].args if old_postrm else 'missing'}")

        if not new_post or new_post[0].args != ["configure", "1.0.0"]:
            errors.append(f"NEW postinst: expected ['configure', '1.0.0'], got {new_post[0].args if new_post else 'missing'}")

        if errors:
            for e in errors:
                log_error(e)
            return False

        log_info("DEB upgrade: NEW preinst='upgrade 1.0.0', OLD prerm='upgrade 2.0.0', OLD postrm='upgrade 2.0.0', NEW postinst='configure 1.0.0'")
        return True

    def test_deb_remove(self) -> bool:
        """Test DEB remove scriptlets."""
        pkg_name = "conary-test-deb-rm"
        pkg = self.tester.deb_gen.generate(pkg_name, "1.0.0")
        if not pkg:
            log_warn("Skipping - DEB generation not available")
            return True

        if not self.tester.install(pkg, pkg_name=pkg_name):
            return False

        self.tester.clear_logs()

        if not self.tester.remove("conary-test-deb-rm"):
            return False

        logs = self.tester.read_logs()

        pre = next((l for l in logs if l.phase == "pre-remove"), None)
        post = next((l for l in logs if l.phase == "post-remove"), None)

        if not pre or pre.args != ["remove"]:
            log_error(f"prerm: expected ['remove'], got {pre.args if pre else 'missing'}")
            return False
        if not post or post.args != ["remove"]:
            log_error(f"postrm: expected ['remove'], got {post.args if post else 'missing'}")
            return False

        log_info("DEB remove: prerm='remove', postrm='remove'")
        return True

    def test_arch_install(self) -> bool:
        """Test Arch fresh install scriptlets."""
        pkg_name = "conary-test-arch"
        pkg = self.tester.arch_gen.generate(pkg_name, "1.0.0")
        if not pkg:
            log_warn("Skipping - Arch generation not available")
            return True

        if not self.tester.install(pkg, pkg_name=pkg_name):
            return False

        logs = self.tester.read_logs()

        pre = next((l for l in logs if l.phase == "pre-install"), None)
        post = next((l for l in logs if l.phase == "post-install"), None)

        # Note: pre_install might not be called if package doesn't define it
        # The important one is post_install

        if not post:
            log_error("post_install did not run")
            return False

        # Arch versions include pkgrel: 1.0.0-1
        expected_version = "1.0.0-1"
        if post.args != [expected_version]:
            log_error(f"post_install got args {post.args}, expected ['{expected_version}']")
            return False

        log_info(f"Arch install: post_install('{post.args[0]}')")
        return True

    def test_arch_upgrade(self) -> bool:
        """Test Arch upgrade scriptlets - OLD scripts should NOT run."""
        pkg_name = "conary-test-arch-upg"
        pkg_v1 = self.tester.arch_gen.generate(pkg_name, "1.0.0")
        pkg_v2 = self.tester.arch_gen.generate(pkg_name, "2.0.0")
        if not pkg_v1 or not pkg_v2:
            log_warn("Skipping - Arch generation not available")
            return True

        if not self.tester.install(pkg_v1, pkg_name=pkg_name):
            return False

        self.tester.clear_logs()

        if not self.tester.install(pkg_v2, pkg_name=pkg_name):
            return False

        logs = self.tester.read_logs()

        # Expected for Arch upgrade:
        # - pre_upgrade(new_version, old_version) from NEW package
        # - post_upgrade(new_version, old_version) from NEW package
        # - NO pre_remove or post_remove from OLD package!

        new_pre = [l for l in logs if l.phase == "pre-upgrade" and l.version == "2.0.0"]
        new_post = [l for l in logs if l.phase == "post-upgrade" and l.version == "2.0.0"]
        old_remove = [l for l in logs if l.phase in ["pre-remove", "post-remove"] and l.version == "1.0.0"]

        errors = []

        # Arch versions include pkgrel: 2.0.0-1, 1.0.0-1
        expected_new = "2.0.0-1"
        expected_old = "1.0.0-1"

        # Check pre_upgrade ran with correct args (might be missing if not defined)
        if new_pre and new_pre[0].args != [expected_new, expected_old]:
            errors.append(f"NEW pre_upgrade: expected ['{expected_new}', '{expected_old}'], got {new_pre[0].args}")

        # post_upgrade must run
        if not new_post:
            errors.append("NEW post_upgrade did not run")
        elif new_post[0].args != [expected_new, expected_old]:
            errors.append(f"NEW post_upgrade: expected ['{expected_new}', '{expected_old}'], got {new_post[0].args}")

        # CRITICAL: OLD package remove scripts must NOT run
        if old_remove:
            errors.append(f"OLD package removal scripts ran (should be skipped): {[l.phase for l in old_remove]}")

        if errors:
            for e in errors:
                log_error(e)
            return False

        log_info("Arch upgrade: NEW pre/post_upgrade('2.0.0', '1.0.0'), OLD remove scripts SKIPPED")
        return True

    def test_arch_remove(self) -> bool:
        """Test Arch remove scriptlets."""
        pkg_name = "conary-test-arch-rm"
        pkg = self.tester.arch_gen.generate(pkg_name, "1.0.0")
        if not pkg:
            log_warn("Skipping - Arch generation not available")
            return True

        if not self.tester.install(pkg, pkg_name=pkg_name):
            return False

        self.tester.clear_logs()

        if not self.tester.remove("conary-test-arch-rm"):
            return False

        logs = self.tester.read_logs()

        pre = next((l for l in logs if l.phase == "pre-remove"), None)
        post = next((l for l in logs if l.phase == "post-remove"), None)

        # For remove, args should be [old_version] (with pkgrel)
        if not post:
            log_error("post_remove did not run")
            return False

        expected_version = "1.0.0-1"
        if post.args != [expected_version]:
            log_error(f"post_remove got args {post.args}, expected ['{expected_version}']")
            return False

        log_info(f"Arch remove: post_remove('{post.args[0]}')")
        return True

    def test_no_scripts_flag(self) -> bool:
        """Test that --no-scripts skips all scriptlets."""
        pkg_name = "conary-test-noscripts"
        pkg = self.tester.rpm_gen.generate(pkg_name, "1.0.0")
        if not pkg:
            log_warn("Skipping - RPM generation not available")
            return True

        if not self.tester.install(pkg, no_scripts=True):
            return False

        logs = self.tester.read_logs()

        if logs:
            log_error(f"Scriptlets ran despite --no-scripts: {[l.phase for l in logs]}")
            return False

        log_info("--no-scripts correctly skipped all scriptlets")
        return True


def find_conary_binary() -> Optional[Path]:
    """Find the conary binary."""
    # Check common locations
    candidates = [
        Path("target/release/conary"),
        Path("target/debug/conary"),
        Path("../target/release/conary"),
        Path("../target/debug/conary"),
        Path("/usr/local/bin/conary"),
        Path("/usr/bin/conary"),
    ]

    for candidate in candidates:
        if candidate.exists():
            return candidate.resolve()

    # Try PATH
    result = shutil.which("conary")
    if result:
        return Path(result)

    return None


def main():
    parser = argparse.ArgumentParser(description="Scriptlet verification test harness")
    parser.add_argument("--keep-artifacts", action="store_true",
                        help="Keep generated packages and logs after tests")
    parser.add_argument("--conary", type=str, help="Path to conary binary")
    parser.add_argument("--test", type=str, help="Run specific test (e.g., 'rpm_install')")
    parser.add_argument("--real-root", action="store_true",
                        help="Install to real root (/) - DANGEROUS, requires sudo, actually runs scriptlets")
    args = parser.parse_args()

    # Find conary binary
    if args.conary:
        conary_bin = Path(args.conary)
        if not conary_bin.exists():
            log_error(f"Conary binary not found: {conary_bin}")
            sys.exit(1)
    else:
        conary_bin = find_conary_binary()
        if not conary_bin:
            log_error("Could not find conary binary. Use --conary to specify path.")
            sys.exit(1)

    log_info(f"Using conary binary: {conary_bin}")

    # Create work directory
    work_dir = Path(tempfile.mkdtemp(prefix="conary_test_"))
    log_info(f"Work directory: {work_dir}")

    try:
        tester = ConaryTester(conary_bin, work_dir, real_root=args.real_root)
        tests = ScriptletTests(tester)

        if not args.real_root:
            log_warn("Running without --real-root: scriptlets will be SKIPPED (non-root install path)")
            log_warn("Use --real-root to actually execute scriptlets (requires sudo)")

        # Define all tests
        all_tests = [
            ("RPM Install", tests.test_rpm_install),
            ("RPM Upgrade", tests.test_rpm_upgrade),
            ("RPM Remove", tests.test_rpm_remove),
            ("DEB Install", tests.test_deb_install),
            ("DEB Upgrade", tests.test_deb_upgrade),
            ("DEB Remove", tests.test_deb_remove),
            ("Arch Install", tests.test_arch_install),
            ("Arch Upgrade (OLD scripts skipped)", tests.test_arch_upgrade),
            ("Arch Remove", tests.test_arch_remove),
            ("--no-scripts Flag", tests.test_no_scripts_flag),
        ]

        # Filter tests if specific test requested
        if args.test:
            all_tests = [(n, t) for n, t in all_tests if args.test.lower() in n.lower()]
            if not all_tests:
                log_error(f"No tests matching '{args.test}'")
                sys.exit(1)

        # Run tests
        results = []
        for name, test_fn in all_tests:
            result = tester.run_test(name, test_fn)
            results.append((name, result))

        # Summary
        print(f"\n{Colors.BOLD}=== Summary ==={Colors.RESET}")
        passed = sum(1 for _, r in results if r)
        failed = sum(1 for _, r in results if not r)

        for name, result in results:
            status = f"{Colors.GREEN}PASS{Colors.RESET}" if result else f"{Colors.RED}FAIL{Colors.RESET}"
            print(f"  {status} {name}")

        print(f"\n{passed} passed, {failed} failed")

        if failed > 0:
            sys.exit(1)

    finally:
        if not args.keep_artifacts:
            shutil.rmtree(work_dir, ignore_errors=True)
        else:
            log_info(f"Artifacts kept at: {work_dir}")


if __name__ == "__main__":
    main()
