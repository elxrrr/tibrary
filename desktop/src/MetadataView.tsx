import { readable } from "./api";

const labels: Record<string, string> = {
  album_id: "Release ID", track_id: "Track ID", id: "Source ID",
  removed_bpm: "BPM in file to remove", retained_bpm: "BPM in retained file",
  removed_key: "Key in file to remove", retained_key: "Key in retained file",
  albumartist: "Album artist", tracknumber: "Track number", tracktotal: "Total tracks", discnumber: "Disc number", disctotal: "Total discs", initialkey: "Musical key", tidal_album_id: "Online release ID", tidal_track_id: "Online track ID",
  bpm: "BPM", key: "Musical key", isrc: "ISRC", upc: "UPC",
};
export const metadataLabel = (key: string) => labels[key] || key.replaceAll("_", " ").replace(/^./, c => c.toUpperCase());

/** Keep source records separate: values from different releases must never look merged. */
export function MetadataView({ value, title }: { value: any; title?: string }) {
  if (value == null || (Array.isArray(value) && !value.length)) return <p className="muted">No saved information.</p>;
  if (Array.isArray(value) && value.some(v => v && typeof v === "object"))
    return <div className="metadata-sources">{value.map((v, i) => <section className="metadata-source" key={i}>
      <h4>{title || "Source"} {i + 1}{v.album_id ? ` · Release ${v.album_id}` : ""}{v.track_id ? ` · Track ${v.track_id}` : ""}</h4>
      <MetadataView value={v}/>
    </section>)}</div>;
  if (typeof value !== "object" || Array.isArray(value)) return <span>{readable(value)}</span>;
  return <div className="metadata-table"><table aria-label={title || "Metadata"}><tbody>
    {Object.entries(value).map(([key, item]) => <tr key={key}><th scope="row">{metadataLabel(key)}</th><td>
      {item && typeof item === "object" && (!Array.isArray(item) || item.some(v => v && typeof v === "object"))
        ? <MetadataView value={item} title={metadataLabel(key)}/> : readable(item)}
    </td></tr>)}
  </tbody></table></div>;
}

export function DownloadReview({ rows }: { rows: any[] }) {
  return <div className="download-review">{rows.map(release => {
    const tracks = (release.children || []).filter((t: any) => release.selected == null || release.selected.includes(t.id));
    return <section className="metadata-source" key={release.id}>
      <h3>{release.artist} — {release.release}</h3>
      <p>{release.date || "Date unavailable"} · {release.type || "Release"} · Release ID {release.id} · {release.selected == null ? `All ${release.tracks} tracks` : `${release.selected.length} selected tracks`}</p>
      {tracks.length ? <div className="metadata-table"><table aria-label={`Tracks in ${release.release}`}>
        <thead><tr><th>Position</th><th>Track</th><th>Track ID</th><th>Duration</th></tr></thead>
        <tbody>{tracks.map((track: any) => <tr key={track.id}><td>{track.position}</td><td>{track.title}</td><td>{track.id}</td><td>{track.duration ? `${Math.floor(track.duration / 60)}:${String(Math.round(track.duration % 60)).padStart(2, "0")}` : "—"}</td></tr>)}</tbody>
      </table></div> : <p>Track names have not been cached yet. {release.selected == null ? "The whole audio release is approved." : `Approved track IDs: ${release.selected.join(", ")}`}</p>}
    </section>;
  })}</div>;
}

export function TagChanges({ changes, current }: { changes: any; current?: any }) {
  if (!changes || typeof changes !== "object" || Array.isArray(changes)) return <MetadataView value={changes}/>;
  return <div className="metadata-table"><table aria-label="Tag comparison"><thead><tr><th>Tag</th><th>Local value</th><th>Proposed value</th></tr></thead><tbody>
    {Object.entries(changes).map(([key, value]) => <tr key={key}><th scope="row">{metadataLabel(key)}</th><td>{readable(current?.[key])}</td><td>{readable(value)}</td></tr>)}
  </tbody></table></div>;
}

export function ExportSummary({ content }: { content: string }) {
  let rows: any[] = [];
  try { const value = JSON.parse(content); if (Array.isArray(value)) rows=value; } catch { return <p>Export is ready to save.</p>; }
  return <div className="metadata-table"><table aria-label="Exported releases"><thead><tr><th>Artist</th><th>Release</th><th>Date</th><th>Release ID</th><th>Selection</th></tr></thead><tbody>
    {rows.map((row,index)=><tr key={row.id || index}><td>{row.release?.artist}</td><td>{row.release?.title}</td><td>{row.release?.date}</td><td>{row.id}</td><td>{row.approved ? "Approved" : "Not approved"}</td></tr>)}
  </tbody></table></div>;
}
