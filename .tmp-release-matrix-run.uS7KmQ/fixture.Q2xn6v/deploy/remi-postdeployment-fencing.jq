def public_profiles:
  ["fedora-44", "ubuntu-26.04", "arch"];

def sha256:
  type == "string" and test("^[0-9a-f]{64}$");

def fencing_epoch($inspection; $profile):
  ([
    $inspection.candidates[]?
    | select(.profile == $profile)
    | .latest_refresh.fencing_epoch
  ] | first) // -1;

def same_fencing_authority($before; $final):
  $before.schema_epoch == $final.schema_epoch
    and $before.schema_revision == $final.schema_revision;

($baseline | length) == 1
  and (
    . as $final
    | $baseline[0] as $before
    | ($before.schema_epoch | type == "string")
      and ($before.schema_revision | type == "number")
      and ($final.schema_epoch | type == "string")
      and ($final.schema_revision | type == "number")
      and ($final.deployment.transition_completed_at | type == "number")
      and ($final.candidate_verification.mode == "publication_attested")
      and ($final.candidate_verification.completed_after
        == $final.deployment.transition_completed_at)
      and ($final.candidate_verification.catalog_files_reopened == 0)
      and ($final.candidate_verification.catalog_bytes_hashed == 0)
      and ($final.candidate_verification.catalog_bytes_integrity_checked == 0)
      and ($final.deployment.repository_refreshes | type == "array")
      and ($final.deployment.repository_refreshes | length) >= 1
      and ($final.deployment.repository_refreshes[0]
        | .scope == {kind: "all"}
          and (.generation | type == "number")
          and .generation >= 1
          and (.finished_at | type == "number")
          and .finished_at > $final.deployment.transition_completed_at)
      and all(public_profiles[];
        . as $profile
        | ([
            $final.candidates[]
            | select(.profile == $profile)
          ]) as $matches
        | ($matches | length) == 1
          and ($matches[0]
            | (.profile_revision_sha256 | sha256)
              and (.run_id | type == "string")
              and (.run_id | length > 0)
              and (.latest_refresh.run_id == .run_id)
              and (.latest_refresh.state == "candidate")
              and (.latest_refresh.fencing_epoch | type == "number")
              and (.latest_refresh.fencing_epoch > 0)
              and (.latest_refresh.started_at | type == "number")
              and (.latest_refresh.finished_at | type == "number")
              and (.latest_refresh.finished_at
                > $final.deployment.transition_completed_at))
          and ($matches[0] as $candidate
            | any($final.deployment.repository_refreshes[];
                (.generation | type == "number")
                and (.started_at | type == "number")
                and (.finished_at | type == "number")
                and .finished_at > $final.deployment.transition_completed_at
                and (.successful_profiles | index($profile)) != null
                and $candidate.latest_refresh.started_at >= .started_at
                and $candidate.latest_refresh.finished_at <= .finished_at))
          and (
            if same_fencing_authority($before; $final) then
              fencing_epoch($final; $profile)
                > fencing_epoch($before; $profile)
            else
              fencing_epoch($final; $profile) > 0
            end
          )
      )
  )
