// crates/conary-core/src/repository/catalog/parity/debian/apt_pkg_shim.cpp

#include <apt-pkg/configuration.h>
#include <apt-pkg/algorithms.h>
#include <apt-pkg/depcache.h>
#include <apt-pkg/deblistparser.h>
#include <apt-pkg/error.h>
#include <apt-pkg/fileutl.h>
#include <apt-pkg/init.h>
#include <apt-pkg/mmap.h>
#include <apt-pkg/pkgcache.h>
#include <apt-pkg/pkgcachegen.h>
#include <apt-pkg/pkgsystem.h>
#include <apt-pkg/sourcelist.h>
#include <apt-pkg/tagfile.h>
#include <apt-pkg/version.h>

#include <algorithm>
#include <cctype>
#include <cstddef>
#include <cstdint>
#include <exception>
#include <iterator>
#include <limits>
#include <memory>
#include <map>
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

struct NativeIdentity {
    std::string name;
    std::string version;
    std::string architecture;
};

struct MissingRequirement {
    NativeIdentity requiring;
    int kind = 0;
    std::string native_text;
};

class ProfilePolicy final : public pkgDepCache::Policy {
  public:
    ProfilePolicy(pkgCache &cache, std::map<std::string, signed short> file_priorities)
        : cache_(cache), file_priorities_(std::move(file_priorities)) {
        for (pkgCache::PkgIterator package = cache_.PkgBegin(); !package.end(); ++package) {
            for (pkgCache::VerIterator version = package.VersionList(); !version.end(); ++version) {
                signed short const priority = file_priority(version);
                if (priority > 0) {
                    version->Priority = static_cast<map_number_t>(30001 - priority);
                }
            }
        }
    }

    pkgCache::VerIterator GetCandidateVer(pkgCache::PkgIterator const &package) override {
        pkgCache::VerIterator selected;
        signed short selected_priority = 0;
        for (pkgCache::VerIterator version = package.VersionList(); !version.end(); ++version) {
            signed short const priority = GetPriority(version);
            if (priority <= 0) {
                continue;
            }
            if (selected.end() || priority > selected_priority ||
                (priority == selected_priority && cache_.VS->CmpVersion(version.VerStr(), selected.VerStr()) > 0)) {
                selected = version;
                selected_priority = priority;
            }
        }
        return selected;
    }

    signed short GetPriority(pkgCache::PkgIterator const &package) override {
        pkgCache::VerIterator candidate = GetCandidateVer(package);
        return candidate.end() ? 0 : GetPriority(candidate);
    }

    signed short GetPriority(pkgCache::VerIterator const &version,
                             bool /*consider_files*/ = true) override {
        return file_priority(version);
    }

  private:
    signed short file_priority(pkgCache::VerIterator const &version) const {
        signed short priority = 0;
        for (pkgCache::VerFileIterator file = version.FileList(); !file.end(); ++file) {
            char const *name = file.File().FileName();
            if (name == nullptr) {
                continue;
            }
            auto const found = file_priorities_.find(name);
            if (found != file_priorities_.end()) {
                priority = std::max(priority, found->second);
            }
        }
        return priority;
    }

    pkgCache &cache_;
    std::map<std::string, signed short> file_priorities_;
};

class EvidenceDepCache final : public pkgDepCache {
  public:
    EvidenceDepCache(pkgCache *cache, Policy *policy) : pkgDepCache(cache, policy) {}

    void allow_exact_root(pkgCache::PkgIterator const &root) {
        exact_root_id_ = root->ID;
        allow_root_ = true;
    }

    void retain_failed_exact_root(pkgCache::VerIterator root) {
        pkgDepCache::StateCache &state = (*this)[root.ParentPkg()];
        state.CandidateVer = root.operator->();
        state.InstallVer = root.operator->();
        state.Mode = ModeInstall;
        state.Status = 2;
        state.iFlags |= Protected;
        Update(nullptr);
    }

    bool IsInstallOk(pkgCache::PkgIterator const &package, bool auto_install = true,
                     unsigned long depth = 0, bool from_user = true) override {
        if (allow_root_ && package->ID == exact_root_id_) {
            return true;
        }
        return pkgDepCache::IsInstallOk(package, auto_install, depth, from_user);
    }

  private:
    map_id_t exact_root_id_ = 0;
    bool allow_root_ = false;
};

struct ResolutionHandle {
    std::unique_ptr<MMap> map;
    std::unique_ptr<pkgCache> cache;
    std::unique_ptr<ProfilePolicy> policy;
    std::vector<Package> packages;
    std::string architecture;
    std::vector<NativeIdentity> closure;
    std::vector<MissingRequirement> missing;
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

Package const *find_source_package(ResolutionHandle const &handle,
                                   pkgCache::VerIterator const &version) {
    std::string const name = version.ParentPkg().Name();
    for (Package const &package : handle.packages) {
        if (package.name == name && package.version == version.VerStr() &&
            package.architecture == version.Arch()) {
            return &package;
        }
    }
    return nullptr;
}

RelationGroup const *strong_group_at(Package const &package, int kind, std::size_t ordinal) {
    std::size_t current = 0;
    for (RelationGroup const &group : package.relations) {
        if (group.kind != kind) {
            continue;
        }
        if (current == ordinal) {
            return &group;
        }
        ++current;
    }
    return nullptr;
}

bool configure_resolution(std::string const &architecture) {
    if (!pkgInitConfig(*_config)) {
        return false;
    }
    _config->Set("APT::Architecture", architecture);
    _config->Set("APT::Architectures", architecture);
    _config->Set("APT::Install-Recommends", "false");
    _config->Set("APT::Install-Suggests", "false");
    _config->Set("APT::Solver", "internal");
    _config->Set("Dir::State::status", "/dev/null");
    _config->Set("Dir::Etc::sourcelist", "/dev/null");
    _config->Set("Dir::Etc::sourceparts", "/dev/null");
    _config->Set("Dir::Cache::pkgcache", "/dev/null");
    _config->Set("Dir::Cache::srcpkgcache", "/dev/null");
    return pkgInitSystem(*_config, _system);
}

bool load_resolution(ResolutionHandle &handle, char const *const *paths, std::size_t path_count) {
    if (paths == nullptr || path_count == 0 ||
        path_count > std::numeric_limits<map_number_t>::max()) {
        handle.error =
            "Debian resolution requires between 1 and 255 ordered Packages objects";
        return false;
    }
    if (!configure_resolution(handle.architecture)) {
        std::string pending = apt_errors();
        handle.error = pending.empty() ? "initialize apt-pkg resolution configuration" : pending;
        return false;
    }

    pkgSourceList sources;
    std::map<std::string, signed short> priorities;
    for (std::size_t ordinal = 0; ordinal < path_count; ++ordinal) {
        char const *path = paths[ordinal];
        if (path == nullptr || *path == '\0') {
            handle.error = "Debian Packages path is empty";
            return false;
        }
        Handle parsed;
        if (!load(parsed, path)) {
            handle.error = parsed.error;
            return false;
        }
        handle.packages.insert(handle.packages.end(),
                               std::make_move_iterator(parsed.packages.begin()),
                               std::make_move_iterator(parsed.packages.end()));
        if (!sources.AddVolatileFile(path)) {
            handle.error = "apt-pkg rejected authenticated Debian Packages object " +
                           std::string(path);
            return false;
        }
        priorities.emplace(path, static_cast<signed short>(30000 - ordinal));
    }

    MMap *map = nullptr;
    if (!pkgCacheGenerator::MakeStatusCache(sources, nullptr, &map, true) || map == nullptr) {
        std::string pending = apt_errors();
        handle.error = pending.empty() ? "build apt-pkg package cache" : pending;
        return false;
    }
    handle.map.reset(map);
    handle.cache = std::make_unique<pkgCache>(handle.map.get());
    if (_error->PendingError()) {
        handle.error = apt_errors();
        return false;
    }
    handle.policy = std::make_unique<ProfilePolicy>(*handle.cache, std::move(priorities));
    return true;
}

pkgCache::VerIterator find_exact_version(ResolutionHandle &handle, std::string const &name,
                                         std::string const &version,
                                         std::string const &architecture) {
    pkgCache::VerIterator selected;
    for (pkgCache::PkgIterator package = handle.cache->PkgBegin(); !package.end(); ++package) {
        if (name != package.Name()) {
            continue;
        }
        for (pkgCache::VerIterator candidate = package.VersionList(); !candidate.end(); ++candidate) {
            if (version == candidate.VerStr() && architecture == candidate.Arch()) {
                if (!selected.end()) {
                    throw std::runtime_error("apt-pkg cache contains ambiguous exact Debian root " +
                                             name + ":" + architecture + "=" + version);
                }
                selected = candidate;
            }
        }
    }
    return selected;
}

bool collect_resolution(ResolutionHandle &handle, pkgDepCache &dependency_cache,
                        pkgCache::VerIterator const &root, bool marked, bool resolved) {
    bool root_selected = false;
    for (pkgCache::PkgIterator package = handle.cache->PkgBegin(); !package.end(); ++package) {
        if (!dependency_cache[package].Install()) {
            continue;
        }
        pkgCache::VerIterator version = dependency_cache[package].InstVerIter(*handle.cache);
        if (version.end()) {
            handle.error = "apt-pkg selected an installed package without a version";
            return false;
        }
        if (version == root) {
            root_selected = true;
        }
        Package const *source = find_source_package(handle, version);
        if (source == nullptr) {
            handle.error = "apt-pkg selected a package absent from authenticated Packages inputs: " +
                           package.FullName(false) + "=" + version.VerStr();
            return false;
        }
        handle.closure.push_back({source->name, source->version, source->architecture});

        std::size_t depends_ordinal = 0;
        std::size_t pre_depends_ordinal = 0;
        for (pkgCache::DepIterator cursor = version.DependsList(); !cursor.end();) {
            pkgCache::DepIterator start;
            pkgCache::DepIterator end;
            cursor.GlobOr(start, end);
            cursor = end;
            ++cursor;

            int kind = 0;
            std::size_t ordinal = 0;
            if (end->Type == pkgCache::Dep::PreDepends) {
                kind = PRE_DEPENDS;
                ordinal = pre_depends_ordinal++;
            } else if (end->Type == pkgCache::Dep::Depends) {
                kind = DEPENDS;
                ordinal = depends_ordinal++;
            }
            bool const satisfied =
                (dependency_cache[end] & pkgDepCache::DepGInstall) == pkgDepCache::DepGInstall;
            if (satisfied) {
                continue;
            }
            if (kind == 0 && !end.IsCritical()) {
                continue;
            }
            if (kind == 0 || end.IsNegative()) {
                handle.error = "apt-pkg found a conflict or unexpected critical dependency for " +
                               package.FullName(false) + "=" + version.VerStr();
                return false;
            }
            bool has_native_target = false;
            for (pkgCache::DepIterator atom = start;; ++atom) {
                std::unique_ptr<pkgCache::Version *[]> targets(atom.AllTargets());
                if (targets[0] != nullptr) {
                    has_native_target = true;
                    break;
                }
                if (atom == end) {
                    break;
                }
            }
            if (has_native_target) {
                handle.error = "apt-pkg could not satisfy a required group with available native "
                               "targets for " +
                               package.FullName(false) + "=" + version.VerStr();
                return false;
            }
            RelationGroup const *group = strong_group_at(*source, kind, ordinal);
            if (group == nullptr) {
                handle.error = "apt-pkg unsatisfied dependency does not bind an exact source group for " +
                               package.FullName(false) + "=" + version.VerStr();
                return false;
            }
            handle.missing.push_back({
                {source->name, source->version, source->architecture}, kind, group->native_text});
        }
    }
    if (!root_selected) {
        handle.error = "apt-pkg did not retain protected exact root " +
                       root.ParentPkg().FullName(false) + "=" + root.VerStr() + " [" +
                       root.Arch() + "]";
        return false;
    }
    if (!handle.missing.empty()) {
        return true;
    }
    if (!marked || !resolved || dependency_cache.BrokenCount() != 0) {
        handle.error = "apt-pkg could not resolve the exact root without a typed missing requirement";
        return false;
    }
    return true;
}

bool resolve(ResolutionHandle &handle, char const *name, char const *version,
             char const *architecture) {
    handle.closure.clear();
    handle.missing.clear();
    handle.error.clear();
    if (name == nullptr || version == nullptr || architecture == nullptr || *name == '\0' ||
        *version == '\0' || *architecture == '\0') {
        handle.error = "Debian exact root identity is incomplete";
        return false;
    }
    if (handle.architecture != architecture && std::string(architecture) != "all") {
        handle.error = "Debian exact root has incompatible target architecture";
        return false;
    }
    pkgCache::VerIterator root =
        find_exact_version(handle, name, version, architecture);
    if (root.end()) {
        handle.error = "Debian exact root is absent from the apt-pkg cache";
        return false;
    }

    EvidenceDepCache dependency_cache(handle.cache.get(), handle.policy.get());
    if (!dependency_cache.Init(nullptr)) {
        handle.error = apt_errors();
        return false;
    }
    dependency_cache.SetCandidateVersion(root);
    bool const marked = dependency_cache.MarkInstall(root.ParentPkg(), true, 0, true, true);
    if (!dependency_cache[root.ParentPkg()].Install()) {
        dependency_cache.allow_exact_root(root.ParentPkg());
        if (!dependency_cache.MarkInstall(root.ParentPkg(), false, 0, true, false)) {
            dependency_cache.retain_failed_exact_root(root);
        }
    }
    bool const provisional = collect_resolution(
        handle, dependency_cache, root, marked, marked && dependency_cache.BrokenCount() == 0);
    if (provisional && !handle.missing.empty()) {
        apt_errors();
        return true;
    }
    handle.closure.clear();
    handle.missing.clear();
    handle.error.clear();
    pkgProblemResolver resolver(&dependency_cache);
    resolver.Protect(root.ParentPkg());
    bool const resolved = resolver.Resolve(false, nullptr);
    std::string const solver_errors = apt_errors();
    if (!collect_resolution(handle, dependency_cache, root, marked, resolved)) {
        if (!solver_errors.empty()) {
            handle.error.append(": ").append(solver_errors);
        }
        return false;
    }
    return true;
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

void *conary_apt_resolution_open(char const *const *paths, std::size_t path_count,
                                 char const *architecture) {
    last_error.clear();
    try {
        if (architecture == nullptr || *architecture == '\0') {
            last_error = "Debian target architecture is empty";
            return nullptr;
        }
        std::unique_ptr<ResolutionHandle> handle = std::make_unique<ResolutionHandle>();
        handle->architecture = architecture;
        if (!load_resolution(*handle, paths, path_count)) {
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
        last_error = "unknown exception while building apt-pkg resolution cache";
        return nullptr;
    }
}

void conary_apt_resolution_free(void *opaque) {
    delete static_cast<ResolutionHandle *>(opaque);
}

int conary_apt_resolution_resolve(void *opaque, char const *name, char const *version,
                                  char const *architecture) {
    auto *handle = static_cast<ResolutionHandle *>(opaque);
    if (handle == nullptr) {
        return 0;
    }
    try {
        return resolve(*handle, name, version, architecture) ? 1 : 0;
    } catch (std::exception const &error) {
        handle->error = error.what();
        std::string pending = apt_errors();
        if (!pending.empty()) {
            handle->error.append(": ").append(pending);
        }
        return 0;
    } catch (...) {
        handle->error = "unknown exception from apt-pkg Debian resolution";
        return 0;
    }
}

char const *conary_apt_resolution_error(void const *opaque) {
    auto const *handle = static_cast<ResolutionHandle const *>(opaque);
    return handle == nullptr ? nullptr : handle->error.c_str();
}

std::size_t conary_apt_resolution_closure_count(void const *opaque) {
    auto const *handle = static_cast<ResolutionHandle const *>(opaque);
    return handle == nullptr ? 0 : handle->closure.size();
}

NativeIdentity const *closure_at(ResolutionHandle const *handle, std::size_t index) {
    return handle == nullptr || index >= handle->closure.size() ? nullptr : &handle->closure[index];
}

#define RESOLUTION_CLOSURE_GETTER(name, member)                                      \
    char const *name(void const *opaque, std::size_t index) {                        \
        NativeIdentity const *identity =                                             \
            closure_at(static_cast<ResolutionHandle const *>(opaque), index);        \
        return identity == nullptr ? nullptr : identity->member.c_str();              \
    }

RESOLUTION_CLOSURE_GETTER(conary_apt_resolution_closure_name, name)
RESOLUTION_CLOSURE_GETTER(conary_apt_resolution_closure_version, version)
RESOLUTION_CLOSURE_GETTER(conary_apt_resolution_closure_architecture, architecture)

std::size_t conary_apt_resolution_missing_count(void const *opaque) {
    auto const *handle = static_cast<ResolutionHandle const *>(opaque);
    return handle == nullptr ? 0 : handle->missing.size();
}

MissingRequirement const *missing_at(ResolutionHandle const *handle, std::size_t index) {
    return handle == nullptr || index >= handle->missing.size() ? nullptr : &handle->missing[index];
}

#define RESOLUTION_MISSING_IDENTITY_GETTER(name, member)                             \
    char const *name(void const *opaque, std::size_t index) {                        \
        MissingRequirement const *missing =                                          \
            missing_at(static_cast<ResolutionHandle const *>(opaque), index);        \
        return missing == nullptr ? nullptr : missing->requiring.member.c_str();      \
    }

RESOLUTION_MISSING_IDENTITY_GETTER(conary_apt_resolution_missing_name, name)
RESOLUTION_MISSING_IDENTITY_GETTER(conary_apt_resolution_missing_version, version)
RESOLUTION_MISSING_IDENTITY_GETTER(conary_apt_resolution_missing_architecture, architecture)

int conary_apt_resolution_missing_kind(void const *opaque, std::size_t index) {
    MissingRequirement const *missing =
        missing_at(static_cast<ResolutionHandle const *>(opaque), index);
    return missing == nullptr ? 0 : missing->kind;
}

char const *conary_apt_resolution_missing_native_text(void const *opaque, std::size_t index) {
    MissingRequirement const *missing =
        missing_at(static_cast<ResolutionHandle const *>(opaque), index);
    return missing == nullptr ? nullptr : missing->native_text.c_str();
}

}  // extern "C"
