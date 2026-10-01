import { describe, expect, it, vi } from "vitest";
import { CoalescedQuery } from "./query";

const deferred = <T>() => {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
};

describe("table reads during background updates", () => {
  it("publishes an in-flight result and coalesces many revisions into one follow-up", async () => {
    vi.useFakeTimers();
    try {
      const first = deferred<string>(), second = deferred<string>();
      const read = vi.fn().mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
      const publish = vi.fn(), loading = vi.fn();
      const query = new CoalescedQuery<string>(loading, vi.fn());
      query.request({key:"queue", revision:1, read, publish});
      await vi.runOnlyPendingTimersAsync();
      for (let revision = 2; revision <= 20; revision++) query.request({key:"queue", revision, read, publish});
      expect(read).toHaveBeenCalledTimes(1);
      first.resolve("cached rows");
      await Promise.resolve();
      expect(publish).toHaveBeenCalledWith("cached rows",1);
      expect(read).toHaveBeenCalledTimes(2);
      second.resolve("updated rows");
      await Promise.resolve();
      expect(publish).toHaveBeenLastCalledWith("updated rows",20);
      expect(loading).toHaveBeenLastCalledWith(false);
      // An on-demand cached detail can change a row without a DB revision.
      const details = deferred<string>(), hydrated = deferred<string>();
      const reread = vi.fn().mockReturnValueOnce(details.promise).mockReturnValueOnce(hydrated.promise);
      query.request({key:"queue",revision:20,version:0,read:reread,publish});
      await vi.runOnlyPendingTimersAsync();
      query.request({key:"queue",revision:20,version:1,read:reread,publish});
      details.resolve("old disclosure");
      await Promise.resolve();
      expect(reread).toHaveBeenCalledTimes(2);
      hydrated.resolve("cached track rows");
      await Promise.resolve();
      expect(publish).toHaveBeenLastCalledWith("cached track rows",20);
    } finally { vi.useRealTimers(); }
  });

  it("loads another page independently and rejects late results for an old view", async () => {
    vi.useFakeTimers();
    try {
      const old = deferred<string>(), current = deferred<string>();
      const publish = vi.fn();
      const query = new CoalescedQuery<string>(vi.fn(), vi.fn());
      query.request({key:"missing", revision:1, read:()=>old.promise, publish});
      await vi.runOnlyPendingTimersAsync();
      query.request({key:"queue", revision:1, read:()=>current.promise, publish});
      await vi.runOnlyPendingTimersAsync();
      current.resolve("queue rows");
      await Promise.resolve();
      old.resolve("old missing rows");
      await Promise.resolve();
      expect(publish).toHaveBeenCalledTimes(1);
      expect(publish).toHaveBeenCalledWith("queue rows",1);
    } finally { vi.useRealTimers(); }
  });

  it("allows retry after failure and prevents publication after leaving the view", async () => {
    vi.useFakeTimers();
    try {
      const publish = vi.fn(), error = vi.fn();
      const query = new CoalescedQuery<string>(vi.fn(), error);
      query.request({key:"queue", revision:1, read:()=>Promise.reject("Failed read"), publish});
      await vi.runOnlyPendingTimersAsync();
      expect(error).toHaveBeenCalledWith("Failed read");
      const retry = deferred<string>();
      query.request({key:"queue", revision:1, read:()=>retry.promise, publish});
      await vi.runOnlyPendingTimersAsync();
      query.clear();
      retry.resolve("rows");
      await Promise.resolve();
      expect(publish).not.toHaveBeenCalled();
    } finally { vi.useRealTimers(); }
  });
});
