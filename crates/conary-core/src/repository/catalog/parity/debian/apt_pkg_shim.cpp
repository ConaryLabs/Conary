// crates/conary-core/src/repository/catalog/parity/debian/apt_pkg_shim.cpp

#include <apt-pkg/configuration.h>
#include <apt-pkg/deblistparser.h>
#include <apt-pkg/error.h>
#include <apt-pkg/fileutl.h>
#include <apt-pkg/init.h>
#include <apt-pkg/pkgcache.h>
#include <apt-pkg/tagfile.h>

#include <algorithm>
#include <cctype>
#include <cstddef>
#include <cstdint>
#include <exception>
#include <memory>
#include <set>
#include <sstream>
#include <stdexcept>
#include <string>
#include <string_view>
#include <utility>
#include <vector>

namespace {

thread_local std::string last_error;

enum RelationKind : int {
    DEPENDS = 1,
    PRE_DEPENDS = 2,
    RECOMMENDS = 3,
    SUGGESTS = 4,
    ENHANCES = 5,
    CONFLICTS = 6,
    BREAKS = 7,
    REPLACES = 8,
};

enum ArchitectureQualifier : int {
    UNQUALIFIED = 0,
    ANY = 1,
    NATIVE = 2,
    EXACT = 3,
};

struct Atom {
    std::string name;
    std::string version;
    std::string native_text;
    std::string architecture;
    int relation = 0;
    int architecture_qualifier = UNQUALIFIED;
    bool continues = false;
};

struct RelationGroup {
    int kind = 0;
    std::string native_text;
    std::vector<Atom> atoms;
};

struct Provide {
    std::string name;
    std::string version;
    std::string native_text;
    std::string architecture;
    int architecture_qualifier = UNQUALIFIED;
};

struct Package {
    std::string name;
    std::string version;
    std::string architecture;
    std::string multi_arch;
    std::string filename;
    std::string sha256;
    std::string size;
    std::vector<Provide> provides;
    std::vector<RelationGroup> relations;
};

struct Handle {
    std::vector<Package> packages;
    std::string error;
};

std::string trim(std::string_view value) {
    while (!value.empty() && std::isspace(static_cast<unsigned char>(value.front())) != 0) {
        value.remove_prefix(1);
    }
    while (!value.empty() && std::isspace(static_cast<unsigned char>(value.back())) != 0) {
        value.remove_suffix(1);
    }
    return std::string(value);
}

std::string apt_errors() {
    std::ostringstream errors;
    _error->DumpErrors(errors);
    return trim(errors.str());
}

bool fail(Handle &handle, std::string message) {
    std::string pending = apt_errors();
    if (!pending.empty()) {
        message.append(": ").append(pending);
    }
    handle.error = std::move(message);
    return false;
}

std::string required_field(pkgTagSection const &section, std::string_view field) {
    std::string value = trim(section.Find(field));
    if (value.empty()) {
        throw std::runtime_error("Debian Packages stanza is missing required " + std::string(field));
    }
    return value;
}

std::string optional_field(pkgTagSection const &section, std::string_view field) {
    return trim(section.Find(field));
}

std::string lower(std::string_view value) {
    std::string result(value);
    std::transform(result.begin(), result.end(), result.begin(), [](unsigned char character) {
        return static_cast<char>(std::tolower(character));
    });
    return result;
}

void reject_repeated_authority(pkgTagSection const &section) {
    static std::set<std::string> const authority_fields = {
        "package", "version", "architecture", "multi-arch", "filename", "sha256", "size",
        "provides", "depends", "pre-depends", "recommends", "suggests", "enhances",
        "conflicts", "breaks", "replaces",
    };
    std::set<std::string> seen;
    for (unsigned int index = 0; index < section.Count(); ++index) {
        char const *start = nullptr;
        char const *stop = nullptr;
        section.Get(start, stop, index);
        if (start == nullptr || stop == nullptr || start >= stop) {
            throw std::runtime_error("apt-pkg returned an invalid deb822 field boundary");
        }
        char const *colon = std::find(start, stop, ':');
        if (colon == stop) {
            throw std::runtime_error("apt-pkg returned a deb822 field without a colon");
        }
        std::string name = lower(trim(std::string_view(start, static_cast<std::size_t>(colon - start))));
        if (authority_fields.find(name) != authority_fields.end() && !seen.insert(name).second) {
            throw std::runtime_error("Debian Packages stanza repeats authority field " + name);
        }
    }
}

void split_architecture(Atom &atom) {
    std::size_t colon = atom.name.rfind(':');
    if (colon == std::string::npos) {
        return;
    }
    std::string qualifier = atom.name.substr(colon + 1);
    atom.name.resize(colon);
    if (qualifier == "any") {
        atom.architecture_qualifier = ANY;
    } else if (qualifier == "native") {
        atom.architecture_qualifier = NATIVE;
    } else {
        atom.architecture_qualifier = EXACT;
        atom.architecture = std::move(qualifier);
    }
    if (atom.name.empty()) {
        throw std::runtime_error("apt-pkg returned an empty qualified package name");
    }
}

std::string render_atom(std::string const &native_name, std::string const &version, unsigned int op) {
    std::string rendered = native_name;
    unsigned int relation = op & 0x0fU;
    if (relation != pkgCache::Dep::NoOp) {
        char const *operator_text = pkgCache::CompTypeDeb(static_cast<unsigned char>(relation));
        if (operator_text == nullptr || *operator_text == '\0' || version.empty()) {
            throw std::runtime_error("apt-pkg returned an incomplete Debian version relation");
        }
        rendered.append(" (").append(operator_text).append(" ").append(version).append(")");
    } else if (!version.empty()) {
        throw std::runtime_error("apt-pkg returned a version without a relation");
    }
    return rendered;
}

std::vector<Atom> parse_atoms(std::string const &field, bool provides) {
    std::vector<Atom> atoms;
    char const *cursor = field.data();
    char const *stop = cursor + field.size();
    while (cursor != stop) {
        std::string native_name;
        std::string version;
        unsigned int op = 0;
        char const *next = debListParser::ParseDepends(
            cursor, stop, native_name, version, op, false, false, false, ""
        );
        if (next == nullptr) {
            throw std::runtime_error("apt-pkg rejected Debian relation field: " + field);
        }
        if (provides && (op & pkgCache::Dep::Or) != 0) {
            throw std::runtime_error("Debian Provides does not permit alternatives: " + field);
        }
        unsigned int relation = op & 0x0fU;
        if (relation == pkgCache::Dep::NotEquals) {
            throw std::runtime_error("Debian binary relations do not support !=: " + field);
        }
        if (provides && relation != pkgCache::Dep::NoOp && relation != pkgCache::Dep::Equals) {
            throw std::runtime_error("Debian Provides permits only an exact version: " + field);
        }
        Atom atom;
        atom.name = native_name;
        atom.version = version;
        atom.relation = static_cast<int>(relation);
        atom.continues = (op & pkgCache::Dep::Or) != 0;
        atom.native_text = render_atom(native_name, version, op);
        split_architecture(atom);
        atoms.push_back(std::move(atom));
        if (next <= cursor) {
            throw std::runtime_error("apt-pkg made no progress parsing Debian relations");
        }
        cursor = next;
    }
    return atoms;
}

void append_relations(Package &package, pkgTagSection const &section, std::string_view field_name,
                      int kind) {
    std::string field = optional_field(section, field_name);
    if (field.empty()) {
        return;
    }
    std::vector<Atom> atoms = parse_atoms(field, false);
    RelationGroup group;
    group.kind = kind;
    for (Atom &atom : atoms) {
        if (!group.native_text.empty()) {
            group.native_text.append(" | ");
        }
        group.native_text.append(atom.native_text);
        bool continues = atom.continues;
        group.atoms.push_back(std::move(atom));
        if (!continues) {
            package.relations.push_back(std::move(group));
            group = RelationGroup{};
            group.kind = kind;
        }
    }
    if (!group.atoms.empty()) {
        throw std::runtime_error("apt-pkg returned an unterminated Debian alternative group");
    }
}

Package parse_package(pkgTagSection const &section) {
    reject_repeated_authority(section);
    Package package;
    package.name = required_field(section, "Package");
    package.version = required_field(section, "Version");
    package.architecture = required_field(section, "Architecture");
    package.multi_arch = optional_field(section, "Multi-Arch");
    package.filename = required_field(section, "Filename");
    package.sha256 = required_field(section, "SHA256");
    package.size = required_field(section, "Size");

    std::string provides_field = optional_field(section, "Provides");
    if (!provides_field.empty()) {
        for (Atom &atom : parse_atoms(provides_field, true)) {
            Provide provide;
            provide.name = std::move(atom.name);
            provide.version = std::move(atom.version);
            provide.native_text = std::move(atom.native_text);
            provide.architecture = std::move(atom.architecture);
            provide.architecture_qualifier = atom.architecture_qualifier;
            package.provides.push_back(std::move(provide));
        }
    }

    append_relations(package, section, "Pre-Depends", PRE_DEPENDS);
    append_relations(package, section, "Depends", DEPENDS);
    append_relations(package, section, "Recommends", RECOMMENDS);
    append_relations(package, section, "Suggests", SUGGESTS);
    append_relations(package, section, "Enhances", ENHANCES);
    append_relations(package, section, "Conflicts", CONFLICTS);
    append_relations(package, section, "Breaks", BREAKS);
    append_relations(package, section, "Replaces", REPLACES);
    return package;
}

bool load(Handle &handle, char const *path) {
    if (path == nullptr || *path == '\0') {
        return fail(handle, "Debian Packages path is empty");
    }
    if (!pkgInitConfig(*_config)) {
        return fail(handle, "initialize apt-pkg configuration");
    }
    FileFd file(path, FileFd::ReadOnly, FileFd::Extension);
    if (!file.IsOpen() || file.Failed()) {
        return fail(handle, "open Debian Packages through apt-pkg");
    }
    pkgTagFile tags(&file, pkgTagFile::STRICT);
    pkgTagSection section;
    while (tags.Step(section)) {
        handle.packages.push_back(parse_package(section));
    }
    if (_error->PendingError()) {
        return fail(handle, "parse Debian Packages through apt-pkg");
    }
    return true;
}

Package const *package_at(Handle const *handle, std::size_t package_index) {
    if (handle == nullptr || package_index >= handle->packages.size()) {
        return nullptr;
    }
    return &handle->packages[package_index];
}

RelationGroup const *group_at(Handle const *handle, std::size_t package_index,
                              std::size_t group_index) {
    Package const *package = package_at(handle, package_index);
    if (package == nullptr || group_index >= package->relations.size()) {
        return nullptr;
    }
    return &package->relations[group_index];
}

Atom const *atom_at(Handle const *handle, std::size_t package_index, std::size_t group_index,
                    std::size_t atom_index) {
    RelationGroup const *group = group_at(handle, package_index, group_index);
    if (group == nullptr || atom_index >= group->atoms.size()) {
        return nullptr;
    }
    return &group->atoms[atom_index];
}

Provide const *provide_at(Handle const *handle, std::size_t package_index,
                          std::size_t provide_index) {
    Package const *package = package_at(handle, package_index);
    if (package == nullptr || provide_index >= package->provides.size()) {
        return nullptr;
    }
    return &package->provides[provide_index];
}

}  // namespace

extern "C" {

char const *conary_apt_pkg_version() { return pkgVersion; }
char const *conary_apt_last_error() { return last_error.c_str(); }

void *conary_apt_open(char const *path) {
    last_error.clear();
    try {
        std::unique_ptr<Handle> handle = std::make_unique<Handle>();
        if (!load(*handle, path)) {
            last_error = handle->error;
            return nullptr;
        }
        return handle.release();
    } catch (std::exception const &error) {
        last_error = error.what();
        std::string pending = apt_errors();
        if (!pending.empty()) {
            last_error.append(": ").append(pending);
        }
        return nullptr;
    } catch (...) {
        last_error = "unknown exception from apt-pkg Debian parser";
        return nullptr;
    }
}

void conary_apt_free(void *opaque) { delete static_cast<Handle *>(opaque); }

std::size_t conary_apt_package_count(void const *opaque) {
    auto const *handle = static_cast<Handle const *>(opaque);
    return handle == nullptr ? 0 : handle->packages.size();
}

#define PACKAGE_STRING_GETTER(name, member)                                                     \
    char const *name(void const *opaque, std::size_t package_index) {                           \
        Package const *package = package_at(static_cast<Handle const *>(opaque), package_index); \
        return package == nullptr ? nullptr : package->member.c_str();                          \
    }

PACKAGE_STRING_GETTER(conary_apt_package_name, name)
PACKAGE_STRING_GETTER(conary_apt_package_version, version)
PACKAGE_STRING_GETTER(conary_apt_package_architecture, architecture)
PACKAGE_STRING_GETTER(conary_apt_package_multi_arch, multi_arch)
PACKAGE_STRING_GETTER(conary_apt_package_filename, filename)
PACKAGE_STRING_GETTER(conary_apt_package_sha256, sha256)
PACKAGE_STRING_GETTER(conary_apt_package_size, size)

std::size_t conary_apt_provide_count(void const *opaque, std::size_t package_index) {
    Package const *package = package_at(static_cast<Handle const *>(opaque), package_index);
    return package == nullptr ? 0 : package->provides.size();
}

#define PROVIDE_STRING_GETTER(name, member)                                                      \
    char const *name(void const *opaque, std::size_t package_index, std::size_t provide_index) { \
        Provide const *provide =                                                                 \
            provide_at(static_cast<Handle const *>(opaque), package_index, provide_index);        \
        return provide == nullptr ? nullptr : provide->member.c_str();                            \
    }

PROVIDE_STRING_GETTER(conary_apt_provide_name, name)
PROVIDE_STRING_GETTER(conary_apt_provide_version, version)
PROVIDE_STRING_GETTER(conary_apt_provide_native_text, native_text)
PROVIDE_STRING_GETTER(conary_apt_provide_architecture, architecture)

int conary_apt_provide_architecture_qualifier(void const *opaque, std::size_t package_index,
                                              std::size_t provide_index) {
    Provide const *provide =
        provide_at(static_cast<Handle const *>(opaque), package_index, provide_index);
    return provide == nullptr ? -1 : provide->architecture_qualifier;
}

std::size_t conary_apt_relation_group_count(void const *opaque, std::size_t package_index) {
    Package const *package = package_at(static_cast<Handle const *>(opaque), package_index);
    return package == nullptr ? 0 : package->relations.size();
}

int conary_apt_relation_group_kind(void const *opaque, std::size_t package_index,
                                   std::size_t group_index) {
    RelationGroup const *group =
        group_at(static_cast<Handle const *>(opaque), package_index, group_index);
    return group == nullptr ? -1 : group->kind;
}

char const *conary_apt_relation_group_native_text(void const *opaque, std::size_t package_index,
                                                  std::size_t group_index) {
    RelationGroup const *group =
        group_at(static_cast<Handle const *>(opaque), package_index, group_index);
    return group == nullptr ? nullptr : group->native_text.c_str();
}

std::size_t conary_apt_relation_atom_count(void const *opaque, std::size_t package_index,
                                           std::size_t group_index) {
    RelationGroup const *group =
        group_at(static_cast<Handle const *>(opaque), package_index, group_index);
    return group == nullptr ? 0 : group->atoms.size();
}

#define ATOM_STRING_GETTER(name, member)                                                        \
    char const *name(void const *opaque, std::size_t package_index, std::size_t group_index,    \
                     std::size_t atom_index) {                                                  \
        Atom const *atom = atom_at(static_cast<Handle const *>(opaque), package_index,          \
                                   group_index, atom_index);                                    \
        return atom == nullptr ? nullptr : atom->member.c_str();                                \
    }

ATOM_STRING_GETTER(conary_apt_relation_atom_name, name)
ATOM_STRING_GETTER(conary_apt_relation_atom_version, version)
ATOM_STRING_GETTER(conary_apt_relation_atom_native_text, native_text)
ATOM_STRING_GETTER(conary_apt_relation_atom_architecture, architecture)

int conary_apt_relation_atom_relation(void const *opaque, std::size_t package_index,
                                      std::size_t group_index, std::size_t atom_index) {
    Atom const *atom = atom_at(static_cast<Handle const *>(opaque), package_index, group_index,
                               atom_index);
    return atom == nullptr ? -1 : atom->relation;
}

int conary_apt_relation_atom_architecture_qualifier(void const *opaque,
                                                    std::size_t package_index,
                                                    std::size_t group_index,
                                                    std::size_t atom_index) {
    Atom const *atom = atom_at(static_cast<Handle const *>(opaque), package_index, group_index,
                               atom_index);
    return atom == nullptr ? -1 : atom->architecture_qualifier;
}

}  // extern "C"
