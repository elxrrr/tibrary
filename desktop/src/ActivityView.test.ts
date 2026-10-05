import { describe, it, expect } from "vitest";
import { compactProgress, workload, mergeActivityHistory, downloadActivity, streamFor, mergeActivitySnapshot, mergeDownloadMonitor, retireWorkerProgress } from "./ActivityView";
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

it("interleaves verbose job details chronologically and reuses saved/live entry identities", () => {
  const saved = [
    {at:"2026-09-27T12:00:00Z",message:"Started scan",job_id:"local",job_kind:"scan",saved_id:1},
    {at:"2026-09-27T12:00:02Z",message:"Release details saved",job_id:"online",job_kind:"discography",saved_id:3},
  ];
  const live = [
    {at:"2026-09-27T12:00:01Z",message:"Downloading track",job_id:"download",job_kind:"download"},
    {...saved[1],saved_id:undefined},
    {at:"2026-09-27T12:00:03Z",message:"Finished scan",job_id:"local",job_kind:"scan",job_status:"complete"},
  ];
  const result=mergeActivityHistory(saved,live);
  expect(result.map(entry=>entry.message)).toEqual(["Finished scan","Release details saved","Downloading track","Started scan"]);
  expect(result.filter(entry=>entry.job_id === "online")).toHaveLength(1);
  const progress={at:"2026-09-27T12:00:04Z",message:"1/4 files",progress_id:"scan"};
  expect(mergeActivityHistory([progress],[{...progress,at:"2026-09-27T12:00:05Z",message:"2/4 files"}])).toEqual([{...progress,at:"2026-09-27T12:00:05Z",message:"2/4 files"}]);
});

it("routes all details and errors by the owning job rather than incidental message words", () => {
  const at = "2026-09-30T12:00:00Z";
  expect(streamFor({at,message:"Download track details failed",category:"error",job_kind:"discography"})).toBe("online");
  expect(streamFor({at,message:"Online title applied",category:"online",job_kind:"apply"})).toBe("local");
  expect(streamFor({at,message:"Catalogue request failed",category:"error",job_kind:"download"})).toBe("downloads");
});

it("updates one artist action without moving its time or resurrecting stale details, and keeps it after completion", () => {
  const artist = {at:"2026-10-05T12:00:00Z",updated_at:"2026-10-05T12:00:01Z",message:"Checking Artist",progress_id:"refresh:artist:123",job_id:"refresh",job_kind:"discography",job_status:"running"};
  const checked = {...artist,updated_at:"2026-10-05T12:00:03Z",message:"Artist · 8 releases checked"};
  const next = {...artist,at:"2026-10-05T12:00:02Z",updated_at:"2026-10-05T12:00:02Z",progress_id:"refresh:artist:456",message:"Checking Next artist"};
  const finished = {at:"2026-10-05T12:00:04Z",message:"Refresh complete",job_id:"refresh",job_kind:"discography",job_status:"complete"};
  const worker = {...artist,progress_id:"refresh",message:"1/2 artists"};
  const snapshot = mergeActivitySnapshot([artist,next,worker],[checked,finished]);
  expect(snapshot.map(entry=>entry.message)).toEqual([checked.message,next.message,finished.message]);
  const saved = mergeActivityHistory(snapshot,[artist]);
  expect(saved.find(entry=>entry.progress_id === artist.progress_id)).toEqual(checked);
  expect(saved.map(entry=>entry.message)).toEqual([finished.message,next.message,checked.message]);
  expect(retireWorkerProgress([...saved,worker])).toEqual(saved);
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
  expect(mergeActivitySnapshot(rows,[],[0,0,0],[1,0,0])).toEqual([]);
  expect(mergeActivitySnapshot([],rows,[1,0,0],[0,0,0])).toEqual([]);
});

it("keeps a flat recent window while older actions remain available through saved history", () => {
  const old = {at:"2026-09-30T10:00:00Z",message:"Files checked",job_id:"old",job_kind:"scan",job_status:"complete"};
  const details = Array.from({length:1100},(_,i)=>({at:new Date(Date.parse("2026-09-30T12:00:00Z")+i*1000).toISOString(),message:`File ${i}`,job_id:"new",job_kind:"scan",job_status:"running"}));
  const rows = mergeActivitySnapshot([old],details);
  expect(rows).toHaveLength(1000);
  expect(rows[0].message).toBe("File 100");
  expect(rows.at(-1)?.message).toBe("File 1099");
  expect(mergeActivityHistory(rows,[old]).some(entry=>entry.job_id === "old")).toBe(true);
});

it("keeps independent transfer lines and prevents late snapshots regressing transfer state", () => {
  const job={id:"download-one",kind:"download",status:"running",message:"Downloading two releases",started:1};
  const items=[{id:"a",kind:"batch",release_id:"r1",job_id:job.id,status:"running"},{id:"b",kind:"batch",release_id:"r2",job_id:job.id,status:"running"},{id:"c",kind:"track",release_id:"r1",job_id:job.id,status:"running"}];
  const transfer=downloadActivity(Object.fromEntries(items.map(item=>[item.id,item])),job);
  expect(transfer).toHaveLength(3);
  expect(new Set(transfer.map(entry=>entry.progress_id)).size).toBe(3);
  expect(transfer.every(entry=>entry.job_id === job.id)).toBe(true);
  const current={track:{id:"a",status:"complete",bytes:1024,updated_at:3}};
  expect(mergeDownloadMonitor(current,{track:{id:"a",status:"running",bytes:512,updated_at:2}})).toEqual(current);
  const monitor={old:{id:"old",kind:"batch",job_id:"previous",total_tracks:20,completed_tracks:20},current:{id:"new",kind:"batch",job_id:job.id,total_tracks:4,completed_tracks:1}};
  expect(workload(job,monitor,2000)?.percent).toBe(25);
});

it("summarizes batch bytes and keeps per-track speed and ETA in the unified log", () => {
  const rows=downloadActivity({batch:{id:"r",kind:"batch",release_id:"r",job_id:"d",artist:"Artist",release:"Release",completed_tracks:1,total_tracks:2,status:"running"},track:{id:"t",kind:"track",release_id:"r",job_id:"d",title:"Track",index:2,total_tracks:2,status:"downloading",percent:50,bytes:1048576,estimated_total_bytes:2097152,bytes_per_second:1048576,eta_seconds:2}});
  expect(rows[0].message).toContain("1/2 tracks · 1.0 MB / ~2.0 MB");
  expect(rows[1].message).toContain("Track 2/2 · 50%");
  expect(rows[1].message).toContain("1.0 MB/s · ~2s remaining");
});
