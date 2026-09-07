"""JSON contracts shared by workflows, model responses, and process handshakes."""
def obj(fields):
    return {"type": "object", "additionalProperties": False, "required": list(fields), "properties": fields}


S = {"type": "string"}
B = {"type": "boolean"}
O = {"type": "object"}
A = {"type": "array"}
STRINGS = {"type": "array", "items": S}
ADVISORY = obj({"id": S, "evidence": S, "fixedVersions": STRINGS})
PLAN = obj({"decision": {"type": "string", "enum": ["remediate", "manual"]}, "sourceSha": S, "reportSha256": S,
            "package": {"type": "string", "enum": ["urllib3"]}, "installedVersion": S, "targetVersion": S,
            "files": STRINGS, "advisories": {"type": "array", "items": ADVISORY}, "compatibility": S, "manualItems": STRINGS})
IMPLEMENTATION = obj({"applied": B, "package": {"type": "string", "enum": ["urllib3"]}, "targetVersion": S, "files": STRINGS})
CONTEXT_FIELDS = {name: S for name in ["decision", "reason", "repository", "sourceSha", "sourceTree", "reportSha256", "imageId",
                                     "databaseSha256", "databaseMetadataSha256", "scannerImage", "package", "installedVersion",
                                     "targetVersion", "residualPolicy", "contractSha256"]}
CONTEXT_FIELDS.update({"files": STRINGS, "targets": A, "sourceFiles": O, "residualCount": {"type": "integer"}})
APPROVED_FIELDS = {**PLAN["properties"], "validated": B, "planSha256": S, "expectedFiles": O}
PATCH_FIELDS = {"ready": B, "sourceSha": S, "sourceTree": S, "planSha256": S, "patchDigest": S, "files": O, "fingerprint": S}
ELIGIBLE_FIELDS = {name: S for name in ["decision", "sourceSha", "sourceTree", "validatedTree", "patchDigest", "fingerprint", "beforeImageId",
                                      "afterImageId", "databaseSha256", "beforeReportSha256", "afterReportSha256", "gateSha256"]}
ELIGIBLE_FIELDS.update({"eligible": {"const": True}, "files": O, "removedAdvisories": STRINGS, "residualFindings": A})
INPUTS = {"capture": obj({}), "validate-plan": obj({"context": obj(CONTEXT_FIELDS), "plan": PLAN}),
          "validate-patch": obj({"context": obj(CONTEXT_FIELDS), "plan": obj(APPROVED_FIELDS), "implementation": IMPLEMENTATION}),
          "eligibility": obj({})}
OUTPUTS = {"capture": obj(CONTEXT_FIELDS), "validate-plan": obj(APPROVED_FIELDS), "validate-patch": obj(PATCH_FIELDS),
           "eligibility": {"oneOf": [obj(ELIGIBLE_FIELDS), obj({"eligible": {"const": False}, "decision": S, "reason": S, "sourceSha": S})]}}
