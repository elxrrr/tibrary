import { describe, it, expect } from "vitest";
import { compactProgress, workload, groupActivity } from "./ActivityView";

describe("catalogue refresh progress", () => {
  it("uses structured completed counts instead of numbers in activity text", () => {
    const result = workload({id:"refresh",kind:"discography",status:"running",message:"Artist 01/02 — release list",started:100,completed:1,total:800}, {}, 101000);
    expect(result?.percent).toBe(0.125);
    expect(compactProgress(result?.percent)).toBe("<1%");
    expect(compactProgress(0)).toBe("0%");
    expect(compactProgress(12.75)).toBe("12%");
    expect(compactProgress(99.9)).toBe("99%");
  });
});

it("shows measured ETA and marks stale or cancelled estimates honestly", () => {
  const job = {id:"x",kind:"link",status:"running",message:"Artist 01/02",started:1,completed:8,total:10,eta_seconds:12,progress_updated_at:10};
  expect(workload(job,{},11000)?.label).toContain("~12s remaining");
  expect(workload(job,{},40000)?.label).toContain("ETA updating");
  expect(workload({...job,status:"cancelling"},{},11000)?.label).toContain("Cancelling");
  expect(workload({...job,status:"complete"},{},11000)).toBeNull();
  expect(workload({...job,completed:undefined,total:undefined},{},11000)?.percent).toBeNull();
});

it("keeps completed job details grouped separately from unrelated messages", () => {
  const at = "2026-09-27T12:00:00Z";
  const result=groupActivity([
    {at,message:"Started",job_id:"a",job_kind:"link",job_status:"running"},
    {at,message:"Needs review · Artist — Track · Partial release match",job_id:"a",job_kind:"link",job_status:"running"},
    {at,message:"Finished",job_id:"a",job_kind:"link",job_status:"complete"},
    {at,message:"Settings saved"},
  ]);
  expect(result.groups).toHaveLength(1);
  expect(result.groups[0].entries).toHaveLength(3);
  expect(result.groups[0].status).toBe("complete");
  expect(result.standalone).toHaveLength(1);
});
