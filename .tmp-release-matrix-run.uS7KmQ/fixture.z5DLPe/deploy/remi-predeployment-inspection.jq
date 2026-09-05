def public_profiles:
  ["arch", "fedora-44", "ubuntu-26.04"];

def sha256:
  type == "string" and test("^[0-9a-f]{64}$");

def string_or_null:
  . == null or type == "string";

def number_or_null:
  . == null or type == "number";

def candidate_identity:
  . == null or (
    (type == "object")
      and (.profile_revision_sha256 | sha256)
      and (.run_id | type == "string")
      and (.completed_at | type == "number")
  );

def refresh_state:
  . == null or (
    (.run_id | type == "string")
      and (.fencing_epoch | type == "number")
      and (.state | type == "string")
      and (.started_at | type == "number")
      and (.heartbeat_at | type == "number")
      and (.finished_at | number_or_null)
      and (.failure_stage | string_or_null)
      and (.failure_category | string_or_null)
      and (.failure_evidence_sha256 == null
        or (.failure_evidence_sha256 | sha256))
      and (.failure_diagnostic | string_or_null)
      and (.run_members | type == "number")
      and (.candidate_members | type == "number")
      and (.redactions | type == "array")
  );

def measurement:
  (.wall_time_micros | type == "number")
    and .wall_time_micros >= 0
    and .wall_time_micros <= 2000000
    and (.user_cpu_micros | type == "number")
    and .user_cpu_micros >= 0
    and (.system_cpu_micros | type == "number")
    and .system_cpu_micros >= 0
    and (.max_rss_bytes | type == "number")
    and .max_rss_bytes > 0
    and (.sqlite_statements | type == "number")
    and .sqlite_statements > 0
    and (.sqlite_page_cache_misses | type == "number")
    and .sqlite_page_cache_misses >= 0
    and (.sqlite_logical_read_bytes | type == "number")
    and .sqlite_logical_read_bytes >= 0
    and .catalog_file_opens == 0
    and .catalog_bytes_read == 0
    and (.output_bytes | type == "number")
    and .output_bytes > 0;

.baseline_schema_version == 1
  and (.schema_epoch | type == "string")
  and (.schema_revision | type == "number")
  and (.candidates | type == "array")
  and .configured_profiles == (public_profiles | length)
  and .candidate_profiles == ([
    .candidates[]
    | select(.identity != null)
  ] | length)
  and ([.candidates[].profile] | sort) == public_profiles
  and all(.candidates[];
    (.configured_sources | type == "number")
      and .configured_sources > 0
      and (.identity | candidate_identity)
      and (.latest_refresh | refresh_state)
  )
  and (.measurement | measurement)
