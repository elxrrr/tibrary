import { describe, expect, it } from "vitest";
import { active, Job, mergeJob } from "./api";

const job = (fields: Partial<Job> = {}): Job => ({
  id: "download-1", kind: "download", status: "running", message: "Downloading", started: 100,
  ...fields,
});

describe("worker progress arriving through events and polling", () => {
  it("does not restart completed downloads when a stale request arrives", () => {
    const completed = job({status: "complete", completed: 3, total: 3});
    expect(mergeJob(completed, job({completed: 1, total: 3}))).toBe(completed);
    expect(mergeJob(completed, null)).toBe(completed);
    expect(active(completed)).toBe(false);
  });

  it("keeps cancellation visible until the selected worker finishes", () => {
    const cancelling = job({status: "cancelling"});
    expect(active(cancelling)).toBe(true);
    expect(mergeJob(cancelling, job())).toBe(cancelling);
    const cancelled = job({status: "cancelled"});
    expect(mergeJob(cancelling, cancelled)).toBe(cancelled);
    expect(active(cancelled)).toBe(false);
  });

  it("keeps newer jobs and progress when older snapshots arrive out of order", () => {
    const newer = job({id: "download-2", started: 200, completed: 2, progress_updated_at: 220});
    expect(mergeJob(newer, job())).toBe(newer);
    expect(mergeJob(newer, {...newer, completed: 1, progress_updated_at: 210})).toBe(newer);
    expect(mergeJob(job(), newer)).toBe(newer);
    const failed = {...newer, status: "failed", message: "Transfer interrupted"};
    expect(mergeJob(newer, failed)).toBe(failed);
  });
});
