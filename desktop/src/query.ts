type Request<T> = {
  key: string;
  revision: number;
  version?: number;
  read: () => Promise<T>;
  publish: (value: T, revision: number) => void;
};

/** Keep a usable response during writes, then coalesce newer revisions once.
 * Changing pages or filters starts an independent read; older views never paint
 * over the current one. Revisions must not repeatedly abandon the same query.
 */
export class CoalescedQuery<T> {
  private current?: Request<T>;
  private running = new Set<string>();
  private timer?: ReturnType<typeof setTimeout>;

  constructor(
    private loading: (value: boolean) => void,
    private error: (error: unknown) => void,
  ) {}

  request(request: Request<T>, delay = 0) {
    this.current = request;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.loading(true);
    if (this.running.has(request.key)) return;
    this.timer = setTimeout(() => {
      this.timer = undefined;
      if (this.current?.key === request.key) void this.start(this.current);
    }, delay);
  }

  clear() {
    this.current = undefined;
    clearTimeout(this.timer);
    this.timer = undefined;
    this.loading(false);
  }

  private async start(request: Request<T>) {
    this.running.add(request.key);
    try {
      const value = await request.read();
      if (this.current?.key === request.key)
        this.current.publish(value, request.revision);
    } catch (error) {
      if (this.current?.key === request.key) this.error(error);
    } finally {
      this.running.delete(request.key);
      if (this.current?.key === request.key) {
        if (this.current.revision !== request.revision || this.current.version !== request.version) void this.start(this.current);
        else this.loading(false);
      }
    }
  }
}
