import { describe, it, expect } from "vitest";
import { compactProgress, workload, groupActivity, groupDownloads, streamFor, mergeActivitySnapshot, mergeDownloadMonitor } from "./ActivityView";
import { mergeJob } from "./api";

it("preserves completion, cancellation and newer progress when request replies arrive late", () => {
  const running={id:"one",kind:"scan",status:"running",message:"Checking files",started:100};
  const complete={...running,status:"complete",finished:110};
  expect(mergeJob(complete,running)).toBe(complete);
  expect(mergeJob(complete,null)).toBe(complete);
  const cancelling={...running,status:"cancelling"};
  expect(mergeJob(cancelling,running)).toBe(cancelling);
  expect(mergeJob(cancelling,complete)).toBe(complete);
  const advanced={...running,completed:20,progress_updated_at:105};
  expect(mergeJob(advanced,running)).toBe(advanced);
  const next={...running,id:"two",started:120};
  expect(mergeJob(complete,next)).toBe(next);
  expect(mergeJob(next,complete)).toBe(next);
});

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

it("routes all details and errors by the owning job rather than incidental message words", () => {
  const at = "2026-09-30T12:00:00Z";
  expect(streamFor({at,message:"Download track details failed",category:"error",job_kind:"discography"})).toBe("online");
  expect(streamFor({at,message:"Online title applied",category:"online",job_kind:"apply"})).toBe("local");
  expect(streamFor({at,message:"Catalogue request failed",category:"error",job_kind:"download"})).toBe("downloads");
});

it("keeps observed details through empty snapshots, late progress and deliberate channel clears", () => {
  const initial = {at:"2026-09-30T12:00:00Z",message:"Started",job_id:"refresh",job_kind:"discography",job_status:"running"};
  const progress = {...initial,at:"2026-09-30T12:00:01Z",message:"Artist 2 of 10",progress_id:"refresh"};
  const advanced = {...progress,at:"2026-09-30T12:00:02Z",message:"Artist 3 of 10"};
  let rows = mergeActivitySnapshot([initial,advanced],[]);
  expect(rows).toHaveLength(2);
  rows = mergeActivitySnapshot(rows,[progress]);
  expect(rows.find(row=>row.progress_id)?.message).toBe(advanced.message);
  const finished = {...initial,at:"2026-09-30T12:00:03Z",message:"Finished",job_status:"complete"};
  rows = mergeActivitySnapshot(rows,[finished]);
  expect(rows.every(row=>!row.progress_id)).toBe(true);
  expect(groupActivity([...rows,progress]).groups[0].status).toBe("complete");
  expect(mergeActivitySnapshot(rows,[],[0,0,0],[1,0,0])).toEqual([]);
  expect(mergeActivitySnapshot([],rows,[1,0,0],[0,0,0])).toEqual([]);
});

it("retains previous job summaries when one verbose job exceeds the recent detail limit", () => {
  const old = {at:"2026-09-30T10:00:00Z",message:"Files checked",job_id:"old",job_kind:"scan",job_status:"complete"};
  const details = Array.from({length:1100},(_,i)=>({at:new Date(Date.parse("2026-09-30T12:00:00Z")+i*1000).toISOString(),message:`File ${i}`,job_id:"new",job_kind:"scan",job_status:"running"}));
  const rows = mergeActivitySnapshot([old],details);
  expect(rows).toHaveLength(1001);
  expect(groupActivity(rows).groups.map(group=>group.id)).toEqual(["new","old"]);
});

it("nests releases under their download job and prevents late snapshots regressing transfer state", () => {
  const job={id:"download-one",kind:"download",status:"running",message:"Downloading two releases",started:1};
  const items=[{id:"a",kind:"batch",release_id:"r1",job_id:job.id,status:"running"},{id:"b",kind:"batch",release_id:"r2",job_id:job.id,status:"running"},{id:"c",kind:"track",release_id:"r1",job_id:job.id,status:"running"}];
  expect(groupDownloads(groupActivity([],job).groups,items,job)).toHaveLength(1);
  expect(groupDownloads([],items)).toHaveLength(1);
  const current={track:{id:"a",status:"complete",bytes:1024,updated_at:3}};
  expect(mergeDownloadMonitor(current,{track:{id:"a",status:"running",bytes:512,updated_at:2}})).toEqual(current);
  const monitor={old:{id:"old",kind:"batch",job_id:"previous",total_tracks:20,completed_tracks:20},current:{id:"new",kind:"batch",job_id:job.id,total_tracks:4,completed_tracks:1}};
  expect(workload(job,monitor,2000)?.percent).toBe(25);
});
