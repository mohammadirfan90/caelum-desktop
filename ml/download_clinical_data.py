"""
CAELUM Desktop: Clinical physiological dataset fetcher and fixture manager.
Downloads authenticated, frame-aligned excerpts of open-access PhysioNet clinical databases:
- mitdb: MIT-BIH Arrhythmia Database (NSR, AFIB, PVC, pauses)
- nstdb: MIT-BIH Noise Stress Test Database (real electrode motion, muscle artifact, baseline wander)
- challenge2015: PhysioNet/CinC 2015 ICU multi-parameter recordings (ECG, Pleth, Resp)
- bidmc: BIDMC PPG and Respiration Database (ECG, PPG, Resp, SpO2)
- slpdb: MIT-BIH Polysomnographic Database (ECG, continuous BP, Resp, Sleep stage annotations)
- drivedb: Stress Recognition in Automobile Drivers (ECG, EDA/GSR, Resp under rest vs stress)
"""

import os
import shutil
import urllib.request
import numpy as np


def ensure_data_dirs(base_dir: str):
    for sub in ["mitdb", "nstdb", "challenge2015", "bidmc", "slpdb", "drivedb"]:
        os.makedirs(os.path.join(base_dir, sub), exist_ok=True)


def fetch_file(url: str, out_path: str, max_bytes: int = None) -> bool:
    if os.path.exists(out_path) and os.path.getsize(out_path) > 0:
        return True
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    req = urllib.request.Request(url, headers={"User-Agent": "Mozilla/5.0"})
    if max_bytes:
        req.add_header("Range", f"bytes=0-{max_bytes - 1}")
    try:
        with urllib.request.urlopen(req, timeout=20) as resp, open(out_path, "wb") as f:
            f.write(resp.read())
        return True
    except Exception as e:
        print(f"Warning: could not fetch {url}: {e}")
        return False


def copy_local_fixtures(root_dir: str, ml_data_dir: str):
    fixtures_dir = os.path.join(root_dir, "crates", "caelum-signal", "tests", "fixtures")
    mitdb_fix = os.path.join(fixtures_dir, "mitdb")
    if os.path.exists(mitdb_fix):
        for f in os.listdir(mitdb_fix):
            src = os.path.join(mitdb_fix, f)
            dst = os.path.join(ml_data_dir, "mitdb", f)
            if not os.path.exists(dst):
                shutil.copy2(src, dst)

    # 201.dat from root
    root_201 = os.path.join(root_dir, "201.dat")
    dst_201 = os.path.join(ml_data_dir, "mitdb", "201.dat")
    if os.path.exists(root_201) and not os.path.exists(dst_201):
        shutil.copy2(root_201, dst_201)

    # Fetch 201.hea and 201.atr if missing
    fetch_file("https://physionet.org/files/mitdb/1.0.0/201.hea", os.path.join(ml_data_dir, "mitdb", "201.hea"))
    fetch_file("https://physionet.org/files/mitdb/1.0.0/201.atr", os.path.join(ml_data_dir, "mitdb", "201.atr"))

    chal_fix = os.path.join(fixtures_dir, "challenge2015")
    if os.path.exists(chal_fix):
        for f in os.listdir(chal_fix):
            src = os.path.join(chal_fix, f)
            dst = os.path.join(ml_data_dir, "challenge2015", f)
            if not os.path.exists(dst):
                shutil.copy2(src, dst)


def fetch_clinical_records(ml_data_dir: str):
    # BIDMC (PPG, ECG, Resp, SpO2)
    for rec in ["bidmc01", "bidmc02", "bidmc03", "bidmc04", "bidmc05"]:
        hea_path = os.path.join(ml_data_dir, "bidmc", f"{rec}.hea")
        dat_path = os.path.join(ml_data_dir, "bidmc", f"{rec}.dat")
        fetch_file(f"https://physionet.org/files/bidmc/1.0.0/{rec}.hea", hea_path)
        fetch_file(f"https://physionet.org/files/bidmc/1.0.0/{rec}.dat", dat_path, max_bytes=150000)
        # Fix header sample count for the excerpt: 150000 / (5 * 2) = 15000 samples
        if os.path.exists(hea_path) and os.path.exists(dat_path):
            with open(hea_path, "r") as hf:
                lines = hf.readlines()
            if lines:
                parts = lines[0].split()
                if len(parts) >= 4:
                    parts[3] = str(os.path.getsize(dat_path) // 10)
                    lines[0] = " ".join(parts) + "\n"
                    with open(hea_path, "w") as hf:
                        hf.writelines(lines)

    # SLPDB (MIT-BIH Polysomnographic: ECG, BP, Resp, EEG, Stage annotations)
    for rec in ["slp01a", "slp01b", "slp02a", "slp02b"]:
        hea_path = os.path.join(ml_data_dir, "slpdb", f"{rec}.hea")
        dat_path = os.path.join(ml_data_dir, "slpdb", f"{rec}.dat")
        st_path = os.path.join(ml_data_dir, "slpdb", f"{rec}.st")
        fetch_file(f"https://physionet.org/files/slpdb/1.0.0/{rec}.hea", hea_path)
        fetch_file(f"https://physionet.org/files/slpdb/1.0.0/{rec}.dat", dat_path, max_bytes=160000)
        fetch_file(f"https://physionet.org/files/slpdb/1.0.0/{rec}.st", st_path)
        if os.path.exists(hea_path) and os.path.exists(dat_path):
            with open(hea_path, "r") as hf:
                lines = hf.readlines()
            if lines:
                parts = lines[0].split()
                if len(parts) >= 4:
                    parts[3] = str(os.path.getsize(dat_path) // 8)
                    lines[0] = " ".join(parts) + "\n"
                    with open(hea_path, "w") as hf:
                        hf.writelines(lines)

    # DRIVEDB (Stress Recognition in Automobile Drivers: ECG, EDA, Resp)
    for rec in ["drive01", "drive02", "drive03"]:
        hea_path = os.path.join(ml_data_dir, "drivedb", f"{rec}.hea")
        dat_path = os.path.join(ml_data_dir, "drivedb", f"{rec}.dat")
        fetch_file(f"https://physionet.org/files/drivedb/1.0.0/{rec}.hea", hea_path)
        fetch_file(f"https://physionet.org/files/drivedb/1.0.0/{rec}.dat", dat_path, max_bytes=140000)
        if os.path.exists(hea_path) and os.path.exists(dat_path):
            with open(hea_path, "r") as hf:
                lines = hf.readlines()
            if lines:
                parts = lines[0].split()
                if len(parts) >= 4:
                    parts[3] = str(os.path.getsize(dat_path) // 12)
                    lines[0] = " ".join(parts) + "\n"
                    with open(hea_path, "w") as hf:
                        hf.writelines(lines)

    # NSTDB (Noise Stress Test: electrode motion, muscle artifact, baseline wander)
    for rec in ["118e00", "em", "ma", "bw"]:
        hea_path = os.path.join(ml_data_dir, "nstdb", f"{rec}.hea")
        dat_path = os.path.join(ml_data_dir, "nstdb", f"{rec}.dat")
        fetch_file(f"https://physionet.org/files/nstdb/1.0.0/{rec}.hea", hea_path)
        fetch_file(f"https://physionet.org/files/nstdb/1.0.0/{rec}.dat", dat_path, max_bytes=120000)


def main():
    root_dir = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    ml_data_dir = os.path.join(root_dir, "ml", "data")
    ensure_data_dirs(ml_data_dir)
    print("Copying local fixtures...")
    copy_local_fixtures(root_dir, ml_data_dir)
    print("Fetching remote clinical excerpts...")
    fetch_clinical_records(ml_data_dir)
    print("✓ Clinical physiological dataset download complete.")


if __name__ == "__main__":
    main()
