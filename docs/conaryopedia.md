# Conaryopedia

*Original documentation from rPath, Inc. - 2012.02.01*

This document serves as a historical reference for the original Conary package management system concepts and architecture. Many of these ideas influenced the design of modern package managers.

---

## Table of Contents

1. [Introduction](#introduction)
2. [Conary Concepts](#1-conary-concepts)
   - [Repositories, Repository Hostnames, and Troves](#11-repositories-repository-hostnames-and-troves)
   - [Labels](#12-labels)
   - [Packages and Components](#13-packages-and-components)
   - [Source Components, Recipes, and Builds](#14-source-components-recipes-and-builds)
   - [Changesets and Revisions](#15-changesets-and-revisions)
   - [Groups](#16-groups)
   - [Version Strings](#17-version-strings)
   - [Flavors](#18-flavors)
3. [Conary System Management](#2-conary-system-management)
   - [Common Conary Commands](#21-common-conary-commands)
   - [Install and Use the Conary Manual Page](#22-install-and-use-the-conary-manual-page)
4. [Recipe Syntax and Structure](#3-recipe-syntax-and-structure)
   - [Recipe Syntax and Naming](#31-recipe-syntax-and-naming)
   - [Recipe Structure](#32-recipe-structure)
5. [Factories](#4-factories)
   - [Developing Packages in rBuilder](#41-developing-packages-in-rbuilder)
   - [Developing Packages Outside rBuilder](#42-developing-packages-outside-rbuilder)
6. [Copying and Customizing Existing Conary Packages](#5-copying-and-customizing-existing-conary-packages)
   - [Shadowing and Deriving](#51-shadowing-and-deriving)
   - [Merging Changes from Upstream](#52-merging-changes-from-upstream)
   - [Cloning and Promoting](#53-cloning-and-promoting)
7. [Appendix A: Recipe Actions, Macros, and Variables](#appendix-a-recipe-actions-macros-and-variables)
8. [Appendix B: Recipe Types and Templates](#appendix-b-recipe-types-and-templates)

---

## Introduction

"Conaryopedia" is the project name for moving Conary documentation to docs.rpath.com. This includes moving many resources from the rPath Wiki (wiki.rpath.com), bringing information up-to-date, and adding new information to the mix.

---

## 1. Conary Concepts

Before you tackle packaging software with Conary, read this chapter to familiarize yourself with some Conary concepts. The sections are presented in a carefully selected order to maximize initial understanding.

### 1.1 Repositories, Repository Hostnames, and Troves

A Conary **repository** is a network-accessible software repository at the heart of Conary's version control features. Conary runs as a service rather than as a data store like APT and YUM repositories. Repositories are important to these types of users in a Conary-based environment:

- The **system administrator** uses commands on an existing Conary-based system to find software and retrieve updates from one or more repositories.
- The **appliance assembler** picks items out of one or more repositories and uses those items to assemble a complete appliance.
- The **packager** checks out repository contents and checks in (or commits) new and modified contents, including software that can be installed and managed with Conary.

Each repository has an identity in Conary called a **repository hostname**. The repository hostname is not a DNS-resolvable hostname, but is an identifier that looks similar to a DNS hostname which Conary maps to a repository URL. This mapping is called a **repository map** (`repositoryMap`) in Conary configuration.

Example repository hostname and its corresponding Conary configuration:

```
demoapp.example.com
repositoryMap demoapp.example.com https://rbuilder.example.com/repos/demoapp
```

A **trove** is an addressable unit in Conary, either in a repository or on a Conary-based system. By "addressable unit," we mean that you can identify a single unique trove by its name, version string, and flavor. Two troves can have the same name but be different versions, or they can be the same name and version but different flavors.

You will see the word "trove" in log files and user interface messages where it serves as a more generic term for referring to packages, components, and groups.

### 1.2 Labels

You can identify where content exists in a Conary repository by using its **label**. A label has the following format:

```
repository_hostname@namespace:tag
```

The three parts of a label are:

- **repository hostname** - The part to the left of the @
- **namespace** - Between the @ and the :
- **tag** - To the right of the colon

The namespace and the tag together make up a **branch name** within that repository.

Example:
```
demoapp.example.com@corp:demoapp-1-devel
```

Refer to troves in a repository as being "on" a particular label. This conveys both which trove you're talking about and where to find it.

For example, the following statement says "the demoapp:runtime trove is on the label demoapp.example.com@corp:demoapp-1-devel":

```
demoapp:runtime=demoapp.example.com@corp:demoapp-1-devel
```

### 1.3 Packages and Components

In Conary package management, a **package** is a trove that represents all the files associated with a single application or customization.

In Conary, a package consists of one or more **components**. A component contains the files that serve a particular functional role in the application. Conary automatically assigns files to components when the package is created.

Components are named with the package name plus the component name, separated by a colon (:), as in `example:lib`.

#### Component Assignment Examples

| Packaged File or Directory | Target Directory | Component Name |
|---------------------------|------------------|----------------|
| example.exe | /usr/bin/ | example:runtime |
| libexample.so | /usr/lib/ | example:lib |
| example/* | /usr/share/doc/ | example:doc |

The role of a component in Conary is to provide efficient dependency resolution when software is installed. A **dependency** is extra software that another application needs in order to function properly.

**Dependency resolution** refers to successfully finding and installing those dependencies.

As a system administrator, you'll see Conary attempt to find and resolve dependencies. Conary identifies dependencies at the file level and resolves dependencies at the component level. This means that Conary installs only the components it needs to provide a given file from a package instead of trying to install the entire package.

The dependency resolution process when installing a trove on a Conary-based system:

1. When you try to install a trove, Conary identifies the trove's dependencies at the file level.
2. For each file it needs, Conary searches the labels that the system is configured to search, looking for the first trove it can find that manages that file.
3. If Conary finds all the files it needs, it reports the components it will need to install. If not, it reports the failure with information about the files it couldn't find.

Conary does not allow any two troves to manage the same file on the same Conary-based system.

### 1.4 Source Components, Recipes, and Builds

A **source component** in Conary is a trove you'll find in a Conary repository that's used to build one or more troves. The source component is named by appending ":source" to the trove name (such as `example:source` for the example package).

A source component contains:
- Instructions for how to build the trove (package or group)
- Other files such as the original application source

The **recipe** file in the package source is the set of instructions used to assemble a package. A recipe is similar to, but much shorter than, a specification file for an RPM.

Conary packaging tools use your package source to **build** the package. The result of building the package is called a **package build**, and that's what you can actually install on a Conary-based system.

> **Note:** Because the word "package" alone might refer to the source, build, or both, this guide uses "package source" and "package build" to help distinguish between them when necessary.

### 1.5 Changesets and Revisions

Troves in a Conary repository can be compared to determine the differences between versions.

Before you can compare two troves, you need to know the **revision** of each trove. The revision is assigned to a trove when a packager checks in or commits that trove to a Conary repository. The revision is made up of three values:

- **upstream version** - The string of characters assigned to the version variable in the package recipe
- **source count** - The revision of the package source (incremented each time a packager commits changes to files in the source component without changing the upstream version)
- **build count** - The revision of the package build (incremented each time a packager commits a new build of the same package source)

Example trove revisions: `1-2-1`, `3.4a-3-2`

Rules Conary uses to assign a revision value:

- If you change the upstream version within the recipe, the source and build counts both reset to 1. (Example: `2.4-3-1` becomes `2.5-1-1`)
- For the same upstream version, if you change the package source, the source count is incremented, and build count resets to 1. (Example: `2.5-1-3` becomes `2.5-2-1`)
- For the same upstream version, if you create a new package build without changing the package source, the build count is incremented. (Example: `2.5-2-1` becomes `2.5-2-2`)
- Source components don't have a build count. (Example: `2.5-2`)

A **changeset** is a file containing a record of differences between one trove and another. During an update, Conary calculates the difference between the package revision on the system and the package revision you're updating to, then creates and applies a changeset.

Conary makes special exceptions when a changeset impacts certain files:
- **Conary-managed configuration files** are merged so the system configuration is preserved
- **Unmanaged files** (not installed and maintained by Conary) are not affected during updates

### 1.6 Groups

In Conary, a **group** is a type of trove whose only purpose is to install and manage other troves. Conary recognizes a group by its naming convention: the `group-` prefix in its name.

As a packager, you can create groups to install and manage a set of troves (packages, components, or even other groups). This is especially useful to lock in specific versions of those troves that you know will work together.

When you install or update a group, Conary will only update the software for a trove in the group if the group itself includes a newer revision of that trove.

A group is created by writing a **group recipe**. Like a package, a group has a source component.

An **appliance group** is a special type of Conary group that defines all the troves needed to build a complete appliance. Thanks to the appliance group build, a system administrator can use a single command to bring the entire system up-to-date.

### 1.7 Version Strings

Each trove is considered unique when it has a unique combination of: a name, a version string, and a flavor.

A **version string** is the complete string of characters that indicates the label and revision of the trove along with any essential branching history from other labels.

Syntax:
```
/label/revision
```

Example:
```
/demoapp.example.com@corp:demoapp-4-devel/4.2-2-1
```

If the trove was branched from one label to another:
```
/parentlabel//label/revision
```

Example (originated on centos.rpath.com@rpath:centos-5 and branched to demoapp.example.com@corp:demoapp-4):
```
/centos.rpath.com@rpath:centos-5//demoapp.example.com@corp:demoapp-4-devel/2.6.1-3.1-1
```

The **branch** is the entire part of the version string from the beginning slash through the last label (remove the last slash and revision).

Two ways to create a new branch for a trove:
- Create the original trove yourself and commit it to a given label
- Derive or shadow an existing trove from another label

### 1.8 Flavors

The same revision of a trove can be built simultaneously for multiple deployment conditions using **flavor specifications**. Each separate flavor specification results in a separate build.

Example flavor specifications:
- `is: x86` - for a 32-bit Intel 8086 processor instruction set
- `vmware` - for a VMware virtual machine

#### Operators for Reading Flavor Specifications

| Operator | Example | Meaning |
|----------|---------|---------|
| (none) | vmware | vmware -- the trove is exclusively built for systems running in VMware |
| ! | !vmware | not vmware -- the trove is exclusively built for systems NOT running in VMware |
| ~ | ~vmware | prefers vmware -- return this trove only if there isn't already a trove that is !vmware |
| ~! | ~!vmware | prefers not vmware -- return this trove only if there isn't already a trove that is vmware |

A flavor is written in square brackets as a comma-separated list, with instruction set flavors listed at the end starting with "is:" and separated by spaces:

```
[!dom0, ~!domU, ~vmware, ~!xen is: x86 x86_64]
```

---

## 2. Conary System Management

Conary has a series of commands for managing software on systems that are based on Conary package management.

> **Note:** All commands that change software on the system require you to be either logged in as root, or using the sudo utility.

### 2.1 Common Conary Commands

#### Get Conary Configuration Information

| Command | Description |
|---------|-------------|
| `conary config` | Display all Conary configuration details in the current scope |
| `conary config \| grep installLabelPath` | Display the installLabelPath setting (sequence of labels the system searches by default) |

#### Get Information About Installed and Available Software

| Command | Description |
|---------|-------------|
| `conary query` | Query installed software for information about its components and update path |
| `conary repquery` | Query software available for installation from Conary repositories |
| `conary verify` | Verify whether changes have been made to installed files |

Examples:
```bash
conary query example
conary q example --info
conary q example --lsl
conary q --path /etc/example.cfg

conary repquery | more
conary rq example --info
conary rq --install-label rap.rpath.com@rpath:linux-2
```

#### Update and Roll Back Software

| Command | Description |
|---------|-------------|
| `conary update` | Install and update software from a Conary repository |
| `conary updateall` | Update the system group and any additional components; `--apply-critical` installs only updates needed by Conary itself |
| `conary migrate` | Migrate all Conary-managed components to a target version of a system group |
| `conary remove` | Remove a file from the system and from Conary management |
| `conary erase` | Uninstall software packages and components |
| `conary rollback` | Roll back a system to a prior state |
| `conary rblist` | List the update operations that would be reversed during each rollback |

Examples:
```bash
conary update example=rap.rpath.com@rpath:linux-2
conary update group-example
conary update changeset.css

conary updateall
conary updateall --resolve
conary updateall --apply-critical
conary updateall --items

conary migrate group-toplevel-appliance

conary remove /usr/share/example/example.doc
conary erase example
conary erase example:runtime

conary rollback 1
conary rollback r.42

conary rblist | more
```

#### Pinned Items (Multiple Versions)

Conary pins kernels so that you can install multiple versions of the same kernel package.

| Command | Description |
|---------|-------------|
| `conary pin` | Pin an installed component or package to prevent modification during updates |
| `conary unpin` | Unpin an installed component or package to allow modification |

Examples:
```bash
conary pin example
conary pin kernel=2.6.17.11-1-0.1
conary unpin example
conary unpin kernel=2.6.17.11-1-0.1
```

### 2.2 Install and Use the Conary Manual Page

Use `man conary` to view the detailed manual page for the conary command.

To install the man utility and conary documentation:
```bash
conary update man=conary.rpath.com@rpl:2 --resolve
conary update conary:doc=conary.rpath.com@rpl:2
```

---

## 3. Recipe Syntax and Structure

The bulk of time you spend packaging is likely to be writing and editing the package recipes. Each application you package has its own requirements to install, set up, and update the software.

You do not need to know how to program in any particular programming language to write a recipe. Most common operations needed for packaging are already in pre-defined recipe actions. You can add custom actions in Python code to your recipe if necessary.

### 3.1 Recipe Syntax and Naming

Recipes are written in the domain-specific language of Conary, which itself is written in Python. Recipes should follow these Python syntax requirements and best practices:

- Use 4-space indentation, not tabs
- Keep line lengths to 78 characters, exceeding this for strings without natural breaks
- Add comments using `#` for single lines or triple-double quotes for multiple lines

```python
# This is a comment in the recipe
"""
This is a longer comment in the recipe.
Python programmers call this a docstring.
"""
```

- When breaking up an action line, indent following lines to the open parenthesis
- When breaking up a long argument over multiple lines, use a single quote at the beginning and end of each line:

```python
r.Configure('options here'
            ' --more-option=here'
            ' --still-more-options')
```

The recipe file is named using the package name plus the `.recipe` extension (e.g., `example.recipe`).

**Filter expressions** are used in policy actions and have the syntax of regular expressions with two special rules:
- A filter expression is always anchored at the beginning
- A trailing slash (/) implies a `.*` immediately following it

### 3.2 Recipe Structure

Within the recipe file, you define a class used to build the package. The class name should be a CamelCase name that reflects the package name.

```python
class ExampleApp(PackageRecipe):
```

Everything after the class declaration line should be indented in 4-space increments.

The basic package recipe has the following possible parts:

- **Class variables** -- used to describe various aspects of the package
- **Processing actions** -- used to direct building the package:
  - **Source actions** -- locate and obtain files for creating the package
  - **Build actions** -- direct compiling software and installing software
  - **Policy actions** -- override Conary default behavior when packaging
- **Use flags and flavors** -- modify how the package is built for certain conditions

#### Example Recipe (tmpwatch from C source code)

```python
class Tmpwatch(CPackageRecipe):
    name = 'tmpwatch'
    version = '2.9.0'

    def setup(r):
        r.addArchive('http://download.example.com/%(name)s-%(version)s.tar.gz')
        r.addSource('crond-tmpwatch', dest='/etc/cron.daily/tmpwatch', mode=0755)
        r.MakeInstall(rootVar='ROOT')
```

The recipe must have at least two class variables:
- **name** - must match the name of your package
- **version** - should reflect the version of the application software (cannot contain hyphens)

Actions are always performed in this order:
1. Source actions (performed first, in order)
2. Build actions (performed second, in order)
3. Policy actions (performed after all source and build actions)

> **Quick reference:** Use `cvc explain <action>` to see the Conary API documentation for a particular recipe action.

---

## 4. Factories

A **factory** is a type of Conary recipe used to generate other recipes. Conary developers created factories so that tools could automatically determine what kind of recipe was needed to package a given piece of software.

When you create a new package for Conary to use a factory, your new recipe automatically inherits all of the code from the factory recipe class. You can create a manifest file that the factory parses to determine where the application files reside.

### 4.1 Developing Packages in rBuilder

If you're developing packages for an appliance in rBuilder, rBuilder automatically assumes you want to use the `FactoryRecipeClass` for your selected platform.

Example factory-based recipe:

```python
# Factory-based recipe for ExamplePkg
class OverrideRecipe(FactoryRecipeClass):

    def preprocess(r):
        '''
        Place pre-processing actions here
        '''

    def postprocess(r):
        '''
        Place post-processing actions here
        '''
```

Unlike most package recipes that have a single method such as `setup()`, this override recipe structure includes two methods: `preprocess` and `postprocess`. Anything added in these methods will be added before or after the factory-generated recipe actions (respectively).

### 4.2 Developing Packages Outside rBuilder

If you're packaging outside of rBuilder, you can still take advantage of a platform's factory recipe classes by imitating the structure of the override recipe above.

The factory name is `factory-capsule-rpm` and is located in `group-factories` in your CentOS, SLES, or RHEL platform repository.

---

## 5. Copying and Customizing Existing Conary Packages

You don't have to start from scratch just to customize a package for your own needs. Instead, you can copy packages and make the changes you want. Conary can automatically keep track of changes upstream, allowing you to merge in those upstream developments at any time.

Three approaches to modifying existing packages:

- **Derive** -- Create a derived package when you want to "drop in" a minor change, like a configuration file or graphic
- **Shadow** -- Create a shadow when you want to make major changes, like recompiling source code with different options
- **Clone** -- Create a clone when you want to make changes without merging from the parent branch over time

### 5.1 Shadowing and Deriving

Both a shadow and a derived package start with a shadowing operation in Conary.

How to decide whether to shadow or derive:

- A **derived package** is sufficient for minor changes (switching out image files, adding a custom configuration)
- A **derived package** starts with the original package as it would install on an existing system, then applies your custom changes
- A **shadow** lets you recompile application source code with different options or build with new requirements
- A **derived package** version must match its parent package, while a shadow's version can increment over time

#### 5.1.1 Derive a Package

The process of creating a derived package:
1. Create a full shadow of the original package (including recipe and all source files)
2. Check out the shadow
3. Remove anything that should stay the same
4. Modify the files that should be different
5. Adjust the recipe to add your customizations

##### Derive with rBuild

```bash
cd example-1/Development
rbuild checkout --derive splashy-theme=conary.rpath.com@rpl:2
cd splashy-theme
# You'll see: CONARY, _ROOT_, splashy-theme.recipe
```

rBuild automatically makes a checkout with your new derived package containing:
- **CONARY** state file (do not modify or remove)
- **_ROOT_** directory with a copy of all files as they would be installed on the target system
- The package recipe file, recreated as a derived package recipe

If you need to modify a file, copy it from `_ROOT_` to your package checkout alongside the recipe file, make your modifications, and add an `r.addSource` line to your recipe.

> **Important:** Do not make changes directly in the `_ROOT_` directory. Leave that directory as the representation of the package you are deriving.

##### Derive with Conary (cvc)

1. Use `cvc shadow` to shadow the package from the "parent" label to your own development label
2. Check out the shadowed package
3. Use `cvc remove` to remove packaged files you do not need to modify
4. Modify the files you do need to modify
5. Modify the recipe:
   - Remove method calls not related to your modifications
   - Add method calls needed to apply changes (e.g., `addSource()`)
   - Change the inherited class to `DerivedPackageRecipe`
   - DO NOT modify the package name or version

Example derived package recipe:

```python
class SplashyTheme(DerivedPackageRecipe):
    name = 'splashy-theme'
    version = '0.3.5'

    def setup(r):
        r.addSource('background.png', dest='%(datadir)s/splashy/themes/background.png')
        r.addSource('background.png', dest='/boot/extlinux/background.png')
```

#### 5.1.2 Shadow a Package

When you use a full shadow, your package build will perform the original build actions used to build the package, just with your modifications added.

##### Shadow with rBuild

```bash
cd example-1/Development
rbuild checkout --shadow splashy=conary.rpath.com@rpl:2
```

##### Shadow with Conary (cvc)

1. Use `cvc shadow` to shadow the package from the "parent" label to your own development label
2. Check out the shadowed package
3. Modify the packaged files as needed

### 5.2 Merging Changes from Upstream

As development continues on the parent branch, you may need to bring in those changes on your own development branch.

#### Compare Packages

**Compare Two Packages (conary):**
```bash
conary rq httpd:runtime=example.rpath.org@corp:devel --file-versions --fullversions
```

**Compare Between Labels in the Repository (cvc):**
```bash
cvc rdiff splashy-theme example.rpath.org@corp:devel conary.rpath.com@rpl:2
```

**Compare Between Your Checkout and a Label:**
```bash
cvc diff conary.rpath.com@rpl:2
```

#### Merge Revisions

From your package checkout:
```bash
cvc merge
```

Or target a specific revision:
```bash
cvc merge 1.0.5
```

On rare occasions, merge conflicts can occur. When this happens, Conary creates a `.conflicts` file for each file that had conflicts.

> **Tip:** After a three-way merge, run `cvc diff` to verify results. If satisfied, commit with `cvc ci`. If not, use `cvc revert` to revert to the last committed state.

### 5.3 Cloning and Promoting

If you don't need to track and merge upstream changes, you can create a new package and copy the contents of the original package.

Two ways to do this:
- Use `rbuild checkout --new` followed by manual efforts to copy files
- (Recommended) Use `cvc clone` to create the cloned package

From that point on, the clone behaves as if you created it. You will not be able to use `cvc merge` on your clone.

**Cloning** creates a sibling relationship between the original package and the clone.

**Promoting** with rBuild:
```bash
rbuild promote
```

**Promoting** with Conary:
```bash
cvc promote group-example example.rpath.org@corp:1-test--example.rpath.org@corp:1
cvc promote group-example @corp:1-test--@corp:1
cvc promote group-example :1-test--:1
```

> **Limitation:** Developers cannot clone between shadow branches that do not share a parent (e.g., `/parentZ//branchA` to `/parentW//branchB`).

---

## Appendix A: Recipe Actions, Macros, and Variables

When creating packages and groups for Conary package management, you can use the Conary Application Programming Interface (API) as a reference for the actions you can include in each recipe.

**Important:** In package recipes, no matter what order your actions appear, they will always occur in this order when you build the package:

1. Source actions
2. Build actions
3. Policy actions

> **Quick reference:** Use `cvc explain <action>` to see API documentation for a particular recipe action.

### A.1 Package and Group Recipe Classes

#### Package Recipe Classes

| Class Name | Description |
|------------|-------------|
| `PackageRecipe` | Base recipe class with all essential requirements |
| `BuildPackageRecipe` | Uses additional build requirements (grep, sed) |
| `CPackageRecipe` | Used for recipes built from C source code |
| `AutoPackageRecipe` | Used for C source code with auto* tools (automake, autoconf) |

#### Other Recipe Classes

| Class Name | Description |
|------------|-------------|
| `GroupRecipe` | For creating Conary groups |
| `DerivedPackageRecipe` | For referencing binary package builds and adding customizations |
| `RedirectRecipe` | For pointing one package/group to others |
| `FilesetRecipe` | Not currently in active development |

### A.2 Package Variables and Actions

Each package must provide values for `name` and `version`:

```python
name = 'example'
version = '1.0a.7'
```

### A.3 Build Requirements and Search Path

**Build requirements** are components needed to perform build operations but not added to the package.

```python
class Example(PackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = [ 'perl:runtime', 'another:component' ]
```

To clear build requirements you don't need:

```python
clearBuildRequires('tar:runtime', 'sed:runtime')
```

To add build requirements not in your search path, use the label:

```python
buildRequires = [ 'perl:runtime', 'another:component=@corp:resource-1' ]
```

### A.4 Macros

Macros are replaced with appropriate values at build time. Conary has several built-in macros.

#### A.4.1 Directory Path Macros

| Macro | Typical Value | Description |
|-------|---------------|-------------|
| `%(bindir)s` | /usr/bin | User executables |
| `%(builddir)s` | (config value) | Source building/compiling operations |
| `%(datadir)s` | /usr/share | Architecture-independent data |
| `%(destdir)s` | (auto) | Installation directory during package build |
| `%(docdir)s` | /usr/share/doc | Miscellaneous documentation |
| `%(essentialbindir)s` | /bin | Essential system binaries |
| `%(essentialsbindir)s` | /sbin | Essential system administration binaries |
| `%(exec_prefix)s` | /usr | Installation prefix for architecture-dependent files |
| `%(includedir)s` | /usr/include | Header files |
| `%(initdir)s` | /etc/init.d | Init scripts for SysVinit |
| `%(lib)s` | lib or lib64 | Library directory name (architecture-dependent) |
| `%(libdir)s` | /usr/lib or /usr/lib64 | Shared data files |
| `%(localstatedir)s` | /var | Dynamic files location |
| `%(mandir)s` | /usr/share/man | Online manual files |
| `%(prefix)s` | /usr | System resources directory |
| `%(sbindir)s` | /usr/sbin | System administration binaries |
| `%(sysconfdir)s` | /etc | System configuration directory |

#### A.4.2 Executable and Option Macros

| Macro | Default Value | Description |
|-------|---------------|-------------|
| `%(cc)s` | gcc | C compiler |
| `%(cxx)s` | g++ | C++ compiler |
| `%(cflags)s` | %(optflags)s %(dbgflags)s | Combined C compiler flags |
| `%(dbgflags)s` | -g | Debug flags |
| `%(optflags)s` | -O2 | C compiler optimization options |
| `%(ldflags)s` | %(dbgflags)s | Linker flags |

#### A.4.3 Build and Cross-compile Macros

| Macro | Description |
|-------|-------------|
| `%(buildbranch)s` | Repository branch associated with package |
| `%(buildlabel)s` | Label of the branch |
| `%(buildcc)s` | C compiler for build system's architecture |
| `%(buildcxx)s` | C++ compiler for build system's architecture |
| `%(sysroot)s` | Alternate system root for cross-compiling |

### A.5 Source Actions

| Action | Description |
|--------|-------------|
| `r.addAction` | Perform shell command during prep stage |
| `r.addArchive` | Add and unpack an archive (tarball, zip) |
| `r.addPatch` | Apply a patch |
| `r.addSource` | Copy a file into the build or destination directory |
| `r.addBzrSnapshot` | Check out from Bazaar VCS |
| `r.addCvsSnapshot` | Check out from CVS |
| `r.addGitSnapshot` | Check out from Git |
| `r.addMercurialSnapshot` | Check out from Mercurial |
| `r.addSvnSnapshot` | Check out from Subversion |
| `r.addPostInstallScript` | Specifies post install script |
| `r.addPostRollbackScript` | Specifies post rollback script |
| `r.addPostUpdateScript` | Specifies post update script |
| `r.addPreRollbackScript` | Specifies pre rollback script |
| `r.addPreUpdateScript` | Specifies pre update script |

### A.6 Build Actions

| Action | Description |
|--------|-------------|
| `r.Ant` | Execute the ant utility |
| `r.Automake` | Runs aclocal, autoconf, and automake |
| `r.ClassPath` | Set the CLASSPATH environment variable |
| `r.CompilePython` | Generate compiled Python files (.pyc, .pyo) |
| `r.Configure` | Run an autoconf configure script |
| `r.ConsoleHelper` | Set up consolehelper symbolic links |
| `r.Copy` | Copy files without changing mode |
| `r.CMake` | Run a cmake configure script |
| `r.Create` | Create a file (empty or with contents) |
| `r.Desktopfile` | Install desktop files |
| `r.Doc` | Install documentation files |
| `r.Environment` | Set an environment variable |
| `r.Install` | Copy files and set their mode |
| `r.JavaCompile` | Run the Java compiler |
| `r.Ldconfig` | Run ldconfig utility |
| `r.Link` | Create a hard link |
| `r.Make` | Run the make utility |
| `r.MakeDirs` | Create directories |
| `r.MakeFIFO` | Create a named pipe (FIFO) |
| `r.MakeInstall` | Run make install |
| `r.ManualConfigure` | Explicitly provide all configure arguments |
| `r.Move` | Move files |
| `r.PythonSetup` | Run setup.py using python-setuptools |
| `r.Remove` | Remove files and directories |
| `r.Replace` | Substitute text in a file |
| `r.Run` | Run a specified shell command |
| `r.SetModes` | Set the mode (permissions) on files |
| `r.Symlink` | Create a symbolic link |
| `r.User` | For user info packages |
| `r.Group` | For group info packages |

### A.7 Policy Actions

Policy actions override Conary's default behavior when packaging.

#### Built-in Policy Actions

| Action | Description |
|--------|-------------|
| `r.ByDefault` | Override default component installation |
| `r.ComponentProvides` | Set package provisions explicitly |
| `r.ComponentRequires` | Create dependencies between components |
| `r.ComponentSpec` | Override default component assignment |
| `r.Config` | Mark files as configuration files |
| `r.ExcludeDirectories` | Prevent deletion of empty directories |
| `r.Flavor` | Mark files as flavor-specific |
| `r.InitialContents` | Mark initial contents files |
| `r.LinkCount` | Control hard-linking between directories |
| `r.MakeDevices` | Create device nodes |
| `r.Ownership` | Set user/group ownership |
| `r.PackageSpec` | Specify package for files (multi-package recipes) |
| `r.Provides` | Mark files as providing features |
| `r.Requires` | Mark files as requiring features |
| `r.TagDescription` | Mark tag description files |
| `r.TagHandler` | Mark tag handler files |
| `r.TagSpec` | Apply tags defined by tag descriptions |
| `r.Transient` | Mark files with transient contents |

#### Pluggable Policy Actions (from conary-policy)

| Action | Description |
|--------|-------------|
| `r.AutoDoc` | Add documentation not otherwise installed |
| `r.BadFilenames` | Require filenames without newlines |
| `r.CheckDesktopFiles` | Warn about errors in desktop files |
| `r.CheckDestDir` | Verify absence of destdir in paths |
| `r.CheckSonames` | Warn about shared library issues |
| `r.DanglingSymlinks` | Disallow dangling symbolic links |
| `r.EnforceSonameBuildRequirements` | Enforce shared library dependencies |
| `r.FixupMultilibPaths` | Repair lib/lib64 path issues |
| `r.NormalizeCompression` | Adjust compressed files for maximum compression |
| `r.NormalizeManPages` | Force manual pages to follow standards |
| `r.PHPRequires` | Determine PHP build requirements |
| `r.PythonEggs` | Verify no .egg files in package |
| `r.RelativeSymlinks` | Change absolute symlinks to relative |
| `r.RemoveNonPackageFiles` | Remove unwanted file types |
| `r.SharedLibrary` | Mark shared libraries for ldconfig |
| `r.Strip` | Remove debugging information |

### A.8 Group Variables and Actions

Group recipes use a special part of the Conary API:

```python
class GroupExample(GroupRecipe):
    name = 'group-example'
    version = '0.1'

    def setup(r):
        # Add items to the group here
```

#### Group Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `depCheck` | False | Check for dependency closure |
| `autoResolve` | False | Include necessary components to resolve dependencies |
| `checkOnlyByDefaultDeps` | True | Check dependencies only in default components |
| `checkPathConflicts` | True | Check for path conflicts |

#### Group Actions

| Action | Description |
|--------|-------------|
| `r.add` | Add a component, package, or group |
| `r.addAll` | Add all directly contained items from a specified group |
| `r.addCopy` | Create a copy and add it |
| `r.addNewGroup` | Add one newly created group to another |
| `r.addResolveSource` | Specify alternate source for resolving dependencies |
| `r.copyComponents` | Add components by copying from another group |
| `r.createGroup` | Create a new subgroup |
| `r.moveComponents` | Add components while removing from another group |
| `r.remove` | Remove a component, package, or group |
| `r.removeComponents` | Indicate components not installed by default |
| `r.removeItemsAlsoInGroup` | Remove duplicates from specified group |
| `r.replace` | Replace a component, package, or group with another |
| `r.Requires` | Create a runtime requirement |
| `r.setByDefault` | Set default additions |
| `r.setDefaultGroup` | Set default group for add/replace actions |
| `r.setLabelPath` | Specify labels to search |
| `r.setSearchPath` | Specify search path (labels and troveSpecs) |

---

## Appendix B: Recipe Types and Templates

When you need to create a new package, select an example recipe as a reference.

> **Note:** Each template shows an empty `buildRequires` list. Conary already has automatic build requirements. After building for the first time, add any build requirements as advised by Conary in the build messages.

### B.1 Binary Executables

For executables that are compiled and ready to launch:

```python
loadSuperClass('binarypackage=conary.rpath.com@rpl:2')
class ExampleApp(BinaryPackageRecipe):
    name = 'exampleapp'
    version = '1.0'
    archive = 'http://www.example.com/exampleapp/%(name)s-%(version)s.tar.bz2'
    buildRequires = []
```

With custom unpack method:

```python
loadSuperClass('binarypackage=conary.rpath.com@rpl:2')
class ExampleApp(BinaryPackageRecipe):
    name = 'exampleapp'
    version = '1.0'
    buildRequires = []

    def unpack(r):
        r.addArchive('http://www.example.com/exampleapp/%(name)s-%(version)s.tar.gz',
                     dir='/opt/%(name)s/', preserveOwnership=True)
```

### B.2 RPM Packages

#### B.2.1 Binary RPM Packages

If using rBuilder's Package Creator, upload your RPM in the web interface. rBuilder combines factories with your uploaded RPM file to generate the recipe code.

#### B.2.2 Source RPM Packages

For packaging from source RPM:

```python
loadSuperClass('rpmpackage=conary.rpath.com@rpl:devel')
class ExampleApp(RPMPackageRecipe, AutoPackageRecipe):
    name = 'example'
    version = '3.1.0'
    rpmRelease = '4'
    rpmPatches = [ 'example-3.0.2.patch', 'example-3.0.5.patch']
    externalArchive = 'http://www.example.com/%(name)s/%(name)s-%(version)s.tar.gz'
```

RPM-specific variables:
- `externalArchive` - Pull archive from a different location
- `tarballName` - Override auto-detected archive name
- `rpmRelease` - RPM release string
- `rpmPatches` - Patches to apply from the RPM
- `rpmSources` - Sources to use from the RPM

### B.3 Java Applications

For pre-compiled Java applications:

```python
loadSuperClass('javapackage=conary.rpath.com@rpl:1')
class ExampleApp(JavaPackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = []

    def upstreamUnpack(r):
        r.addArchive('http://www.example.com/%(example)s/%(example)s-%(version)s.tgz')
```

### B.4 C Source Code

For C source code with simple ./configure; make; make install:

```python
class ExampleApp(AutoPackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = []

    def unpack(r):
        r.addArchive('http://www.example.com/%(name)s-sources/%(name)s-%(version)s.tgz')
```

For C source code with additional adjustments:

```python
class ExampleApp(CPackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = []

    def setup(r):
        r.addArchive('http://www.example.com/%(name)s-sources/%(name)s-%(version)s.tgz')
        r.Make(makeName = 'Makefile-differentname')
        r.Run('%(builddir)s/pre-install.sh')
        r.MakeInstall()
        r.Install('final.exe', '%(datadir)s/%(name)s/')
```

### B.5 Python Source Code

For Python applications:

```python
class ExampleApp(PackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = [ 'python-setuptools:python' ]

    def setup(r):
        r.addArchive('http://www.example.com/%(name)s/%(name)s-%(version)s.tar.bz2')
        r.PythonSetup()
```

For Python with site-packages:

```python
loadInstalled('python')
class ExampleApp(PackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = [ 'python-setuptools:python' ]

    def setup(r):
        r.macros.pyver = Python.majversion
        r.macros.sitepkgs = '%(libdir)s/python%(pyver)s/site-packages'
        r.addArchive('http://www.example.com/%(name)s/%(name)s-%(version)s.tar.bz2')
        r.PythonSetup()
```

If PythonSetup fails with `--single-version-externally-managed not recognized`:

```python
r.Run('python setup.py build')
r.Run('python setup.py install --root=%(destdir)s')
```

### B.6 PHP Applications

For PHP applications:

```python
class ExampleApp(PackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = []

    def setup(r):
        r.addArchive('http://www.example.com/%(name)s/'
                     '%(name)s-%(version)s.tar.bz2', dir='%(servicedir)s/%(name)s/')
        r.SetModes('%(servicedir)s/%(name)s/Sources', 0755)
        r.SetModes('%(servicedir)s/%(name)s/index.php', 0644)
        r.Ownership('apache', 'root', '%(servicedir)s/%(name)s/Sources')
        r.ExcludeDirectories(exceptions = '%(servicedir)s/%(name)s/Plugins')
```

### B.7 Ruby Source Code

For Ruby source code with setup.rb:

```python
class ExampleApp(PackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = []

    def setup(r):
        r.addArchive('http://www.example.com/example/'
                     '%(name)s-%(version)s.tar.bz2',
                     dir='/opt/%(name)s/')
        r.Run('ruby setup.rb config')
        r.Run('ruby setup.rb setup')
        r.Run('ruby setup.rb install --prefix="%(destdir)s"')
```

### B.8 Perl Applications from CPAN

For Perl applications from CPAN:

```python
loadSuperClass('cpanpackage=conary.rpath.com@rpl:2')
class ExampleApp(CPANPackageRecipe):
    name = 'perl-example'
    version = '1.0'
    author = 'CPANAUTHORNAME'
    server = 'http://search.cpan.org/CPAN/'
    buildRequires = []
```

If the Perl module builds a C library, also inherit from CPackageRecipe:

```python
class ExampleApp(CPANPackageRecipe, CPackageRecipe):
```

### B.9 Custom Linux Kernel Packages

Start by shadowing `kernel:source` from your platform.

For rPath Linux (already built from kernel source):

```python
loadSuperClass('kernelpackage=conary.rpath.com@rpl:devel')
class Kernel(KernelPackageRecipe):
    name = 'kernel'
    version = '2.6.15.3'

    def unpack(r):
        r.addPatch('my-custom-feature.patch')
```

Kernel flavor specifications:
- `x86` and `x86_64` - architecture flavors
- `smp` - SMP support (`kernel.smp` or `!kernel.smp`)
- `pae` - PAE support
- `numa` - NUMA support

### B.10 GNOME Applications

For GNOME desktop applications:

```python
loadRecipe('gnomepackage.recipe')
class ExampleApp(GnomePackageRecipe):
    name = 'example'
    version = '1.0'
    buildRequires = []
    extraConfig = '--disable-scrollkeeper'
```

### B.11 Group Recipes

For a group of packages and components:

```python
class GroupExample(GroupRecipe):
    name = 'group-example'
    version = '0.1'
    autoResolve = True

    def setup(r):
        r.add('example:runtime', '/example.rpath.org@corp:example-4/4.0.1')
        r.add('mysql', '5.0.51a')
        r.add('php', '5.2.8')
```

Important facts about Conary groups:
- Packages in a group are installed and managed together ("lock-in" feature)
- If you update an individual package, it's no longer managed by the group
- Recommended practice: update the group recipe and rebuild the group

### B.12 Creating System Users and Groups with Info Packages

For creating a Linux system user:

```python
class info_appuser(UserInfoRecipe):
    name = 'info-appuser'
    version = '1'

    def setup(r):
        r.User('appuser', 1001, group='examplegroup', groupid=1001,
               homedir='/home/appuser', shell='%(essentialbindir)s/bash',
               supplemental=['wheel','mysql'])
```

For creating a Linux system group:

```python
class info_examplegroup(GroupInfoRecipe):
    name = 'info-examplegroup'
    version = '1'

    def setup(r):
        r.Group('examplegroup', 999)
```

> **Warning:** Removing an info- package with `conary erase` does not delete the entry from /etc/passwd.

### B.13 Redirect Packages and Groups

For redirecting a package or group to a different location:

```python
class Example(RedirectRecipe):
    name = 'example'
    version = '0'

    def setup(r):
        r.addRedirect('perl-Mail-SpamAssassin', 'conary.rpath.com@rpl:devel')
```

For multiple targets:

```python
class Example(RedirectRecipe):
    name = 'example'
    version = '0'
    allowMultipleTargets = True

    def setup(r):
        target_list = [ 'example-server', 'example-client', 'example-common']
        for x in target_list:
            r.addRedirect(x, 'example.rpath.org@corp:example-4')
```

### B.14 Encapsulate an Existing RPM

For encapsulating an RPM as a capsule package:

```python
class FoopkgRecipe(CapsuleRecipe):
    name = 'foopkg'
    version = '1.0'

    def setup(r):
        r.addCapsule('http://example.com/foopkg.rpm')
```

For multiple architectures:

```python
class FoopkgRecipe(CapsuleRecipe):
    name = 'foopkg'
    version = '1.0'

    def setup(r):
        r.addCapsule('foopkg.x86_64.rpm', use=Arch.x86_64)
        r.addCapsule('foopkg.i586.rpm', use=Arch.x86)
```

---

*This document is based on original documentation Copyright 2012 rPath, Inc. Converted for historical reference purposes.*
