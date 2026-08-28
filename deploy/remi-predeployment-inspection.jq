def public_profiles:
  ["arch", "fedora-44", "ubuntu-26.04"];

def sha256:
  type == "string" and test("^[0-9a-f]{64}$");

def string_or_null:
  . == null or type == "string";

def number_or_null:
  . == null or type == "number";

def candidate_identity:
  if .profile_revision_sha256 == null then
    .run_id == null
      and .completed_at == null
      and .packages == 0
  else
    (.profile_revision_sha256 | sha256)
      and (.run_id | type == "string")
      and (.completed_at | type == "number")
      and (.packages | type == "number")
      and .packages > 0
  end;

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

(.schema_epoch | type == "string")
  and (.schema_revision | type == "number")
  and (.candidates | type == "array")
  and .configured_profiles == (public_profiles | length)
  and .candidate_profiles == ([
    .candidates[]
    | select(.profile_revision_sha256 != null)
  ] | length)
  and ([.candidates[].profile] | sort) == public_profiles
  and all(.candidates[];
    (.configured_sources | type == "number")
      and .configured_sources > 0
      and candidate_identity
      and (.latest_refresh | refresh_state)
  )
