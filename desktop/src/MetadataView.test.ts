import {describe, expect, it} from "vitest";
import {catalogueFieldStatus, creditCoverage} from "./MetadataView";

describe("saved metadata coverage", () => {
  it("distinguishes a summary or partial track list from fully checked empty credits", () => {
    expect(creditCoverage({tracks_loaded:false, track_count:2}).complete).toBe(false);
    expect(creditCoverage({tracks_loaded:true, track_count:2, tracks:[{credits_complete:true, credits:[]}]}).complete).toBe(false);
    expect(creditCoverage({tracks_loaded:true, track_count:2, tracks:[{credits_complete:true, credits:[]}, {credits_complete:true, credits:[]}]})).toEqual({checked:2, total:2, loaded:true, complete:true});
  });
  it("does not call populated but unchecked credits complete", () => {
    expect(creditCoverage({tracks_loaded:true, track_count:1, tracks:[{credits:[{name:"Composer",role:"Writer"}]}]}).checked).toBe(0);
  });
  it("separates unavailable catalogue values from unchecked or failed lookups", () => {
    expect(catalogueFieldStatus({}, "genres")).toBe("Not checked");
    const release = {catalogue_metadata_status:{fields:{genres:"not_supplied",label:"incomplete",providers:"supplied"}}};
    expect(catalogueFieldStatus(release,"genres")).toBe("Checked · not supplied by the service");
    expect(catalogueFieldStatus(release,"label")).toBe("Check incomplete · update needed");
    expect(catalogueFieldStatus(release,"providers")).toBe("Saved");
  });
});
