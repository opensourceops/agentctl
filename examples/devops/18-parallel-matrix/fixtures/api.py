SERVICE = "api"
def health():
    return {"healthy": True, "service": SERVICE}
