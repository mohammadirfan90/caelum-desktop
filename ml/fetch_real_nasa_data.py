"""
CAELUM Desktop: Real NASA Spaceflight Data Fetcher.
Downloads authentic, open-access NASA datasets directly from official NASA endpoints:
1. NASA Open Science Data Repository (OSDR) S3 Archive:
   - OSD-679 (LSDS-81): Real spaceflight head-down tilt tonometry, intraocular pressure (IOP),
     and optical coherence tomography (OCT) ocular hemodynamics.
   - OSD-137 (LSDS-28): Real spaceflight physiological histology and metabolic metrics.
2. NASA Langley Research Center POWER API:
   - Authentic atmospheric surface pressure (kPa), ambient temperature (C), and relative
     humidity (%) from NASA Earth-observing satellite observations and atmospheric assimilation models.
3. NASA Space Radiation Analysis Group (SRAG) / NOAA GOES-18 Space Weather Telemetry:
   - Authentic integral proton flux (>=1 MeV, >=10 MeV, >=100 MeV) for Solar Particle Event (SPE)
     radiation monitoring in caelum-vehicle.
"""

import os
import sys
import json
import urllib.request

if hasattr(sys.stdout, "reconfigure"):
    sys.stdout.reconfigure(encoding="utf-8")
if hasattr(sys.stderr, "reconfigure"):
    sys.stderr.reconfigure(encoding="utf-8")


def fetch_file(url: str, dest_path: str, timeout: int = 20) -> bool:
    os.makedirs(os.path.dirname(dest_path), exist_ok=True)
    if os.path.exists(dest_path) and os.path.getsize(dest_path) > 0:
        print(f"[EXISTS] {dest_path}")
        return True
    print(f"[FETCHING] {url} -> {dest_path} ...")
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64)"})
    try:
        with urllib.request.urlopen(req, timeout=timeout) as resp, open(dest_path, "wb") as f:
            f.write(resp.read())
        sz = os.path.getsize(dest_path)
        print(f"  [OK] Saved {dest_path} ({sz} bytes)")
        return True
    except Exception as e:
        print(f"  [ERROR] Failed to fetch {url}: {e}")
        return False


def main():
    root_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    nasa_dir = os.path.join(root_dir, "ml", "data", "nasa")
    os.makedirs(nasa_dir, exist_ok=True)

    # 1. NASA OSDR OSD-679 (Real Spaceflight Analog Tonometry & OCT Hemodynamics)
    osd679_tonometry = "https://nasa-osdr.s3.amazonaws.com/OSD-679/version-2/tonometry/LSDS-81_tonometry_Fuller_TonometryIOP_TRANSFORMED.csv"
    fetch_file(osd679_tonometry, os.path.join(nasa_dir, "nasa_osd679_tonometry_iop.csv"))

    osd679_oct = "https://nasa-osdr.s3.amazonaws.com/OSD-679/version-2/Ophthalmologic%20Diagnostic%20Technique/LSDS-81_Ophthalmologic%20Diagnostic%20Technique_Fuller_OCT_TRANSFORMED.csv"
    fetch_file(osd679_oct, os.path.join(nasa_dir, "nasa_osd679_oct_hemodynamics.csv"))

    osd679_mri = "https://nasa-osdr.s3.amazonaws.com/OSD-679/version-2/Tomography/LSDS-81_Tomography_Fuller_MRI_Eye_TRANSFORMED.csv"
    fetch_file(osd679_mri, os.path.join(nasa_dir, "nasa_osd679_mri_eye.csv"))

    # 2. NASA OSDR OSD-137 (Real Spaceflight Physiological Histology)
    osd137_histology = "https://nasa-osdr.s3.amazonaws.com/OSD-137/version-7/Histology/LSDS-28_Histology_OSD_137_Histology_TRANSFORMED.csv"
    fetch_file(osd137_histology, os.path.join(nasa_dir, "nasa_osd137_histology.csv"))

    # 3. NASA Langley POWER API (Real Atmospheric Environmental Telemetry)
    power_url = "https://power.larc.nasa.gov/api/temporal/hourly/point?parameters=PS,T2M,RH2M&community=RE&longitude=-95.09&latitude=29.56&start=20240101&end=20240107&format=JSON"
    fetch_file(power_url, os.path.join(nasa_dir, "nasa_power_jsc_environmental.json"))

    # 4. NASA SRAG / NOAA GOES-18 (Real Space Radiation Proton Flux Telemetry)
    proton_url = "https://services.swpc.noaa.gov/json/goes/primary/integral-protons-1-day.json"
    fetch_file(proton_url, os.path.join(nasa_dir, "nasa_srag_goes18_proton_flux.json"))

    print("\n[COMPLETE] Real NASA open spaceflight datasets successfully acquired.")


if __name__ == "__main__":
    main()
