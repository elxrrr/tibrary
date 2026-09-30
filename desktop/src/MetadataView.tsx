import { readable } from "./api";

const labels: Record<string, string> = {
  album_id: "Release ID", track_id: "Track ID", id: "Source ID",
  removed_bpm: "BPM in file to remove", retained_bpm: "BPM in retained file",
  removed_key: "Key in file to remove", retained_key: "Key in retained file",
  albumartist: "Album artist", tracknumber: "Track number", tracktotal: "Total tracks", discnumber: "Disc number", disctotal: "Total discs", initialkey: "Musical key", tidal_album_id: "Online release ID", tidal_track_id: "Online track ID",
  bpm: "BPM", key: "Musical key", isrc: "ISRC", upc: "UPC",
};
export const metadataLabel = (key: string) => labels[key] || key.replaceAll("_", " ").replace(/^./, c => c.toUpperCase());

export function creditCoverage(release: any) {
  const tracks = Array.isArray(release?.tracks) ? release.tracks : [];
  const checked = tracks.filter((track: any) => track.credits_complete === true).length;
  const expected = Number(release?.track_count || (typeof release?.tracks === "number" ? release.tracks : tracks.length));
  const loaded = release?.tracks_loaded === true && tracks.length > 0;
  return {checked, total:Math.max(expected, tracks.length), loaded,
    complete:loaded && checked === tracks.length && tracks.length >= expected};
}

function metadataText(value: any): string {
  return readable(value?.text ?? value);
}

export function catalogueFieldStatus(release: any, field: string): string {
  const status = release?.catalogue_metadata_status?.fields?.[field];
  if (status === "supplied") return "Saved";
  if (status === "not_supplied") return "Checked · not supplied by the service";
  if (status === "incomplete") return "Check incomplete · update needed";
  return "Not checked";
}

/** Display the actual saved online evidence, including successful empty checks. */
export function ReleaseMetadata({release, track}: {release: any; track?: any}) {
  const coverage = creditCoverage(release);
  const fields: [string, any][] = [
    ["Album artist", release.album_artist || release.artist],
    ["Release date", release.date], ["Original release date", release.original_release_date],
    ["UPC", release.upc], ["Record label", release.label],
    ["Provider / distributor", release.providers?.map((provider: any) => provider.name).filter(Boolean).join("; ") || release.provider_name],
    ["Copyright", release.copyright], ["Genres", release.genres],
    ["Audio modes", release.audio_modes], ["Quality", release.quality],
  ];
  const tracks: any[] = track ? [track] : Array.isArray(release.tracks) ? release.tracks : [];
  return <section className="release-metadata">
    <h3>Saved online metadata</h3>
    <p className={coverage.complete ? "success" : "warning"}>
      {coverage.complete ? `Track details and credits checked · ${coverage.checked}/${coverage.total} tracks`
        : coverage.loaded ? `Credits checked · ${coverage.checked}/${coverage.total} tracks · remaining evidence needs an update`
        : "Release summary only · track details and credits have not been loaded"}
    </p>
    <p className="muted">Missing values are not proof of a mismatch. A completed credit check can return no credits; record labels and genres are separate catalogue fields.</p>
    <div className="metadata-table"><table aria-label="Saved release metadata"><tbody>
      {fields.map(([label, value]) => <tr key={label}><th scope="row">{label}</th><td>
        {value == null || value === "" || (Array.isArray(value) && !value.length) ? "Not supplied in saved metadata" : metadataText(value)}
      </td></tr>)}
    </tbody></table></div>
    <div className="metadata-table"><table aria-label="Catalogue checks"><thead><tr><th>Catalogue field</th><th>Check status</th></tr></thead><tbody>
      {[["genres", "Genres"], ["label", "Record label"], ["providers", "Provider / distributor"], ["replacement", "Replacement release"]].map(([field, label]) =>
        <tr key={field}><th scope="row">{label}</th><td>{catalogueFieldStatus(release, field)}</td></tr>)}
    </tbody></table></div>
    {release.catalogue_metadata_status?.checked_at && <p className="muted">Catalogue fields checked {new Date(release.catalogue_metadata_status.checked_at * 1000).toLocaleString()}</p>}
    {release.metadata_note && <p className="warning">{release.metadata_note}</p>}
    {release.track_metadata_checked_at && <p className="muted">Track details checked {new Date(release.track_metadata_checked_at * 1000).toLocaleString()}</p>}
    <div className="metadata-sources">{tracks.map((recording: any) => <details key={recording.id} className="metadata-source">
      <summary>{recording.title} · Disc {String(recording.disc_number || 1).padStart(2, "0")} · Track {String(recording.track_number || "?").padStart(2, "0")} · {recording.credits_complete ? "Credits checked" : "Credits not checked"}</summary>
      <p>Track ID {recording.id} · ISRC {recording.isrc || "Not supplied"} · BPM {recording.bpm || "Not supplied"} · Key {recording.key || "Not supplied"}{recording.key_scale ? ` ${recording.key_scale}` : ""}</p>
      {recording.artists?.length > 0 && <p>Performer credits: {recording.artists.map((artist: any) => `${artist.name || "Unnamed artist"}${artist.type ? ` (${artist.type.toLowerCase()})` : ""}`).join("; ")}</p>}
      {recording.copyright && <p>Copyright: {metadataText(recording.copyright)}</p>}
      {recording.genres?.length > 0 && <p>Genres: {readable(recording.genres)}</p>}
      {Array.isArray(recording.credits) && recording.credits.length ? <div className="metadata-table"><table aria-label={`Credits for ${recording.title}`}>
        <thead><tr><th>Role</th><th>Contributor</th><th>Contributor ID</th></tr></thead><tbody>
          {recording.credits.map((credit: any, index: number) => <tr key={index}><td>{credit.role || credit.roleId || "Not supplied"}</td><td>{credit.name || credit.person || "Not supplied"}</td><td>{credit.contributor_id ?? "Not supplied"}</td></tr>)}
        </tbody></table></div> : <p>{recording.credits_complete ? "Checked successfully; the service supplied no credits for this track." : "Credits have not been checked. Get missing metadata to complete this evidence."}</p>}
    </details>)}</div>
  </section>;
}

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
    {Object.entries(value).filter(([key, item]) => {
      const canonical: Record<string,string> = {album_artist:"albumartist",track_artist:"artist",track_number:"tracknumber",disc_number:"discnumber",track_total:"tracktotal",disc_total:"disctotal"};
      const alias = canonical[key]; return !alias || value[alias] == null || readable(value[alias]) !== readable(item);
    }).map(([key, item]) => <tr key={key}><th scope="row">{metadataLabel(key)}</th><td>
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
