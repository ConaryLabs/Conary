/* crates/conary-core/src/repository/catalog/parity/rpm/libsolv_shim.c */

#include <errno.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#include <solv/dataiterator.h>
#include <solv/knownid.h>
#include <solv/pool.h>
#include <solv/repo.h>
#include <solv/repo_rpmmd.h>
#include <solv/problems.h>
#include <solv/queue.h>
#include <solv/solver.h>
#include <solv/solv_xfopen.h>
#include <solv/solvable.h>
#include <solv/solvversion.h>
#include <solv/transaction.h>

typedef struct {
    Pool *pool;
    Id *packages;
    uint32_t *members;
    size_t package_count;
    size_t package_capacity;
    Solver *solver;
    Transaction *transaction;
    Queue closure;
    Queue problem_rules;
    int fileprovides_added;
    char error[512];
} ConarySolv;

typedef struct {
    Dataiterator iterator;
    int initialized;
} ConarySolvFileIterator;

enum {
    CONARY_SOLV_PROVIDES = 1,
    CONARY_SOLV_REQUIRES = 2,
    CONARY_SOLV_CONFLICTS = 3,
    CONARY_SOLV_OBSOLETES = 4,
    CONARY_SOLV_RECOMMENDS = 5,
    CONARY_SOLV_SUGGESTS = 6,
    CONARY_SOLV_SUPPLEMENTS = 7,
    CONARY_SOLV_ENHANCES = 8,
};

static void
set_error(ConarySolv *handle, const char *operation, const char *detail)
{
    snprintf(handle->error, sizeof(handle->error), "%s: %s", operation,
             detail && *detail ? detail : "libsolv returned an unspecified error");
}

static Solvable *
package_at(ConarySolv *handle, size_t index)
{
    if (!handle || !handle->pool || index >= handle->package_count)
        return NULL;
    return handle->pool->solvables + handle->packages[index];
}

static size_t
package_index_for_id(ConarySolv *handle, Id package_id)
{
    if (!handle || package_id <= 0)
        return SIZE_MAX;
    for (size_t index = 0; index < handle->package_count; index++)
        if (handle->packages[index] == package_id)
            return index;
    return SIZE_MAX;
}

static void
clear_resolution(ConarySolv *handle)
{
    if (!handle)
        return;
    if (handle->transaction) {
        transaction_free(handle->transaction);
        handle->transaction = NULL;
    }
    if (handle->solver) {
        solver_free(handle->solver);
        handle->solver = NULL;
    }
    queue_empty(&handle->closure);
    queue_empty(&handle->problem_rules);
}

static int
package_provides_dependency(Solvable *solvable, Id dependency)
{
    if (!solvable || !solvable->repo || !solvable->provides)
        return 0;
    for (Offset offset = solvable->provides;
         solvable->repo->idarraydata[offset] != 0; offset++)
        if (solvable->repo->idarraydata[offset] == dependency)
            return 1;
    return 0;
}

static int
add_exact_file_providers(ConarySolv *handle, Id dependency)
{
    if (!handle || !handle->pool || dependency <= 0 || ISRELDEP(dependency))
        return 0;
    const char *path = pool_id2str(handle->pool, dependency);
    if (!path || *path != '/')
        return 0;
    /*
     * pool_addfileprovides handles normal RPMMD file dependencies first.
     * Some complete filelists extensions are not indexed by that helper, so
     * a typed missing file rule triggers one exact scan of libsolv's own
     * reopened filelist data before the root is declared unresolved.
     */
    Dataiterator iterator;
    dataiterator_init(&iterator, handle->pool, NULL, 0, SOLVABLE_FILELIST,
                      NULL, SEARCH_FILES);
    int added = 0;
    while (dataiterator_step(&iterator)) {
        if (!iterator.kv.str || strcmp(iterator.kv.str, path) != 0)
            continue;
        Id package_id = iterator.solvid;
        if (package_id <= 0 || package_id >= handle->pool->nsolvables)
            continue;
        Solvable *solvable = handle->pool->solvables + package_id;
        if (!solvable->repo || package_provides_dependency(solvable, dependency))
            continue;
        solvable->provides = repo_addid_dep(solvable->repo, solvable->provides,
                                            dependency, SOLVABLE_FILEMARKER);
        added++;
    }
    dataiterator_free(&iterator);
    return added;
}

static int
add_problem_file_providers(ConarySolv *handle)
{
    if (!handle || !handle->solver)
        return 0;
    int added = 0;
    Id problem = 0;
    while ((problem = solver_next_problem(handle->solver, problem)) != 0) {
        Queue rules;
        queue_init(&rules);
        solver_findallproblemrules(handle->solver, problem, &rules);
        for (int index = 0; index < rules.count; index++) {
            Id from = 0;
            Id to = 0;
            Id dependency = 0;
            SolverRuleinfo info = solver_ruleinfo(
                handle->solver, rules.elements[index], &from, &to, &dependency);
            if (info == SOLVER_RULE_PKG_NOTHING_PROVIDES_DEP)
                added += add_exact_file_providers(handle, dependency);
        }
        queue_free(&rules);
    }
    return added;
}

static Offset
dependency_offset(Solvable *solvable, int field)
{
    if (!solvable)
        return 0;
    switch (field) {
    case CONARY_SOLV_PROVIDES:
        return solvable->provides;
    case CONARY_SOLV_REQUIRES:
        return solvable->requires;
    case CONARY_SOLV_CONFLICTS:
        return solvable->conflicts;
    case CONARY_SOLV_OBSOLETES:
        return solvable->obsoletes;
    case CONARY_SOLV_RECOMMENDS:
        return solvable->recommends;
    case CONARY_SOLV_SUGGESTS:
        return solvable->suggests;
    case CONARY_SOLV_SUPPLEMENTS:
        return solvable->supplements;
    case CONARY_SOLV_ENHANCES:
        return solvable->enhances;
    default:
        return 0;
    }
}

static int
append_repo_packages(ConarySolv *handle, Repo *repo, uint32_t member)
{
    size_t required = handle->package_count + (size_t)repo->nsolvables;
    if (required > handle->package_capacity) {
        size_t capacity = handle->package_capacity ? handle->package_capacity : 1024;
        while (capacity < required) {
            if (capacity > SIZE_MAX / 2) {
                set_error(handle, "index RPM packages", "package count overflow");
                return 0;
            }
            capacity *= 2;
        }
        Id *packages = realloc(handle->packages, capacity * sizeof(*packages));
        if (!packages) {
            set_error(handle, "index RPM packages", strerror(errno));
            return 0;
        }
        handle->packages = packages;
        uint32_t *members = realloc(handle->members, capacity * sizeof(*members));
        if (!members) {
            set_error(handle, "index RPM packages", strerror(errno));
            return 0;
        }
        handle->members = members;
        handle->package_capacity = capacity;
    }

    Id id;
    Solvable *solvable;
    FOR_REPO_SOLVABLES(repo, id, solvable) {
        handle->packages[handle->package_count] = id;
        handle->members[handle->package_count] = member;
        handle->package_count++;
    }
    return 1;
}

ConarySolv *
conary_solv_create(void)
{
    ConarySolv *handle = calloc(1, sizeof(*handle));
    if (!handle)
        return NULL;
    handle->pool = pool_create();
    if (!handle->pool) {
        free(handle);
        return NULL;
    }
    pool_setdisttype(handle->pool, DISTTYPE_RPM);
    queue_init(&handle->closure);
    queue_init(&handle->problem_rules);
    return handle;
}

void
conary_solv_free(ConarySolv *handle)
{
    if (!handle)
        return;
    clear_resolution(handle);
    queue_free(&handle->closure);
    queue_free(&handle->problem_rules);
    pool_free(handle->pool);
    free(handle->packages);
    free(handle->members);
    free(handle);
}

const char *
conary_solv_version(void)
{
    return solv_version;
}

const char *
conary_solv_error(ConarySolv *handle)
{
    return handle ? handle->error : "libsolv handle is null";
}

int
conary_solv_load_rpmmd(ConarySolv *handle, const char *name,
                       const char *primary_path, const char *filelists_path,
                       uint32_t member, int precedence)
{
    if (!handle || !name || !primary_path || !filelists_path)
        return 0;
    if (handle->fileprovides_added) {
        set_error(handle, "load RPM repository",
                  "repositories cannot be added after solver preparation");
        return 0;
    }
    handle->error[0] = '\0';
    Repo *repo = repo_create(handle->pool, name);
    if (!repo) {
        set_error(handle, "create RPM repository", pool_errstr(handle->pool));
        return 0;
    }
    repo->priority = precedence;

    FILE *primary = solv_xfopen(primary_path, "r");
    if (!primary) {
        set_error(handle, "open RPM primary metadata", strerror(errno));
        return 0;
    }
    int result = repo_add_rpmmd(repo, primary, NULL, 0);
    fclose(primary);
    if (result != 0) {
        set_error(handle, "parse RPM primary metadata", pool_errstr(handle->pool));
        return 0;
    }

    FILE *filelists = solv_xfopen(filelists_path, "r");
    if (!filelists) {
        set_error(handle, "open RPM filelists metadata", strerror(errno));
        return 0;
    }
    result = repo_add_rpmmd(repo, filelists, NULL, REPO_EXTEND_SOLVABLES);
    fclose(filelists);
    if (result != 0) {
        set_error(handle, "parse RPM filelists metadata", pool_errstr(handle->pool));
        return 0;
    }
    repo_internalize(repo);
    return append_repo_packages(handle, repo, member);
}

int
conary_solv_set_architecture(ConarySolv *handle, const char *architecture)
{
    if (!handle || !architecture || !*architecture)
        return 0;
    clear_resolution(handle);
    pool_setarch(handle->pool, architecture);
    return 1;
}

int
conary_solv_solve(ConarySolv *handle, size_t root_index)
{
    if (!handle || root_index >= handle->package_count)
        return -1;
    clear_resolution(handle);
    handle->error[0] = '\0';
    if (!handle->fileprovides_added) {
        pool_addfileprovides(handle->pool);
        handle->fileprovides_added = 1;
    }
    for (;;) {
        handle->solver = solver_create(handle->pool);
        if (!handle->solver) {
            set_error(handle, "create RPM solver", "allocation failed");
            return -1;
        }
        solver_set_flag(handle->solver, SOLVER_FLAG_IGNORE_RECOMMENDED, 1);
        solver_set_flag(handle->solver, SOLVER_FLAG_STRICT_REPO_PRIORITY, 1);

        Queue jobs;
        queue_init(&jobs);
        queue_push2(&jobs, SOLVER_INSTALL | SOLVER_SOLVABLE,
                    handle->packages[root_index]);
        int result = solver_solve(handle->solver, &jobs);
        queue_free(&jobs);
        if (result == 0)
            break;
        if (add_problem_file_providers(handle)) {
            solver_free(handle->solver);
            handle->solver = NULL;
            pool_freewhatprovides(handle->pool);
            continue;
        }
        Id problem = 0;
        while ((problem = solver_next_problem(handle->solver, problem)) != 0) {
            Queue rules;
            queue_init(&rules);
            solver_findallproblemrules(handle->solver, problem, &rules);
            for (int index = 0; index < rules.count; index++)
                queue_pushunique(&handle->problem_rules, rules.elements[index]);
            queue_free(&rules);
        }
        if (!handle->problem_rules.count) {
            set_error(handle, "solve RPM root", "problems carried no typed rules");
            return -1;
        }
        return 0;
    }

    handle->transaction = solver_create_transaction(handle->solver);
    if (!handle->transaction) {
        set_error(handle, "create RPM transaction", "allocation failed");
        return -1;
    }
    transaction_installedresult(handle->transaction, &handle->closure);
    return 1;
}

size_t
conary_solv_closure_count(ConarySolv *handle)
{
    return handle ? (size_t)handle->closure.count : 0;
}

size_t
conary_solv_closure_package_index(ConarySolv *handle, size_t index)
{
    if (!handle || index >= (size_t)handle->closure.count)
        return SIZE_MAX;
    return package_index_for_id(handle, handle->closure.elements[index]);
}

size_t
conary_solv_problem_rule_count(ConarySolv *handle)
{
    return handle ? (size_t)handle->problem_rules.count : 0;
}

int
conary_solv_problem_rule(ConarySolv *handle, size_t index, int *type,
                         size_t *from_index, size_t *to_index, int *dependency)
{
    if (!handle || !handle->solver || index >= (size_t)handle->problem_rules.count)
        return 0;
    Id from = 0;
    Id to = 0;
    Id dep = 0;
    SolverRuleinfo info = solver_ruleinfo(
        handle->solver, handle->problem_rules.elements[index], &from, &to, &dep);
    if (type)
        *type = (int)info;
    if (from_index)
        *from_index = package_index_for_id(handle, from);
    if (to_index)
        *to_index = package_index_for_id(handle, to);
    if (dependency)
        *dependency = dep;
    return 1;
}

int
conary_solv_required_kind(ConarySolv *handle, size_t package_index,
                          int dependency)
{
    Solvable *solvable = package_at(handle, package_index);
    if (!solvable || !solvable->requires || dependency == 0)
        return 0;
    int prerequisite = 0;
    int found = 0;
    for (Offset offset = solvable->requires;
         solvable->repo->idarraydata[offset] != 0; offset++) {
        Id current = solvable->repo->idarraydata[offset];
        if (current == SOLVABLE_PREREQMARKER) {
            prerequisite = 1;
            continue;
        }
        if (current == dependency) {
            int kind = prerequisite ? 2 : 1;
            if (found && found != kind)
                return -1;
            found = kind;
        }
    }
    return found;
}

size_t
conary_solv_package_count(ConarySolv *handle)
{
    return handle ? handle->package_count : 0;
}

uint32_t
conary_solv_package_member(ConarySolv *handle, size_t index)
{
    return handle && index < handle->package_count ? handle->members[index] : UINT32_MAX;
}

const char *
conary_solv_package_name(ConarySolv *handle, size_t index)
{
    Solvable *solvable = package_at(handle, index);
    return solvable ? pool_id2str(handle->pool, solvable->name) : NULL;
}

const char *
conary_solv_package_arch(ConarySolv *handle, size_t index)
{
    Solvable *solvable = package_at(handle, index);
    return solvable ? pool_id2str(handle->pool, solvable->arch) : NULL;
}

const char *
conary_solv_package_evr(ConarySolv *handle, size_t index)
{
    Solvable *solvable = package_at(handle, index);
    return solvable ? pool_id2str(handle->pool, solvable->evr) : NULL;
}

const char *
conary_solv_package_location(ConarySolv *handle, size_t index)
{
    Solvable *solvable = package_at(handle, index);
    return solvable ? solvable_lookup_location(solvable, NULL) : NULL;
}

const char *
conary_solv_package_checksum(ConarySolv *handle, size_t index, int *is_sha256)
{
    Solvable *solvable = package_at(handle, index);
    if (!solvable)
        return NULL;
    Id type = 0;
    const char *checksum = solvable_lookup_checksum(solvable, SOLVABLE_CHECKSUM, &type);
    if (is_sha256)
        *is_sha256 = type == REPOKEY_TYPE_SHA256;
    return checksum;
}

uint64_t
conary_solv_package_size(ConarySolv *handle, size_t index, int *found)
{
    Solvable *solvable = package_at(handle, index);
    if (!solvable) {
        if (found)
            *found = 0;
        return 0;
    }
    unsigned long long missing = UINT64_MAX;
    unsigned long long value = solvable_lookup_num(solvable, SOLVABLE_DOWNLOADSIZE, missing);
    if (found)
        *found = value != missing;
    return value;
}

size_t
conary_solv_dependency_count(ConarySolv *handle, size_t index, int field)
{
    Solvable *solvable = package_at(handle, index);
    Offset offset = dependency_offset(solvable, field);
    if (!solvable || !offset)
        return 0;
    size_t count = 0;
    while (solvable->repo->idarraydata[offset + count] != 0)
        count++;
    return count;
}

int
conary_solv_dependency_at(ConarySolv *handle, size_t index, int field,
                          size_t dependency_index)
{
    Solvable *solvable = package_at(handle, index);
    Offset offset = dependency_offset(solvable, field);
    if (!solvable || !offset)
        return 0;
    size_t current = 0;
    while (solvable->repo->idarraydata[offset + current] != 0) {
        if (current == dependency_index)
            return solvable->repo->idarraydata[offset + current];
        current++;
    }
    return 0;
}

int
conary_solv_dependency_is_relation(int dependency)
{
    return ISRELDEP(dependency);
}

int
conary_solv_dependency_flags(ConarySolv *handle, int dependency)
{
    if (!handle || !ISRELDEP(dependency))
        return 0;
    return GETRELDEP(handle->pool, dependency)->flags;
}

int
conary_solv_dependency_name(ConarySolv *handle, int dependency)
{
    if (!handle || !ISRELDEP(dependency))
        return 0;
    return GETRELDEP(handle->pool, dependency)->name;
}

int
conary_solv_dependency_evr(ConarySolv *handle, int dependency)
{
    if (!handle || !ISRELDEP(dependency))
        return 0;
    return GETRELDEP(handle->pool, dependency)->evr;
}

const char *
conary_solv_dependency_atom(ConarySolv *handle, int dependency)
{
    if (!handle || dependency == 0 || ISRELDEP(dependency))
        return NULL;
    return pool_id2str(handle->pool, dependency);
}

const char *
conary_solv_dependency_text(ConarySolv *handle, int dependency)
{
    if (!handle || dependency == 0)
        return NULL;
    return pool_dep2str(handle->pool, dependency);
}

int
conary_solv_dependency_is_prereq_marker(int dependency)
{
    return dependency == SOLVABLE_PREREQMARKER;
}

ConarySolvFileIterator *
conary_solv_file_iterator(ConarySolv *handle, size_t index)
{
    Solvable *solvable = package_at(handle, index);
    if (!solvable)
        return NULL;
    ConarySolvFileIterator *files = calloc(1, sizeof(*files));
    if (!files)
        return NULL;
    Id package_id = handle->packages[index];
    dataiterator_init(&files->iterator, handle->pool, solvable->repo,
                      package_id, SOLVABLE_FILELIST, NULL, SEARCH_FILES);
    files->initialized = 1;
    return files;
}

const char *
conary_solv_file_next(ConarySolvFileIterator *files)
{
    if (!files || !files->initialized)
        return NULL;
    while (dataiterator_step(&files->iterator)) {
        if (files->iterator.kv.str)
            return files->iterator.kv.str;
    }
    return NULL;
}

void
conary_solv_file_iterator_free(ConarySolvFileIterator *files)
{
    if (!files)
        return;
    if (files->initialized)
        dataiterator_free(&files->iterator);
    free(files);
}
