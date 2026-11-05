//! Bounded descriptor reads for static project configuration; never executes project tools.
use std::{fs::OpenOptions, io::{self, Read}, path::Path};
const LIMIT:u64=4*1024*1024;
pub(crate) fn read(path:&Path)->io::Result<String> {
    let mut options=OpenOptions::new();options.read(true);
    #[cfg(unix)] {use std::os::unix::fs::OpenOptionsExt;options.custom_flags(libc::O_NONBLOCK);}
    let file=options.open(path)?;let before=file.metadata()?;
    if !before.is_file(){return Err(io::Error::new(io::ErrorKind::InvalidData,"Configuration must be a regular file"));}
    if before.len()>LIMIT{return Err(io::Error::new(io::ErrorKind::InvalidData,"Configuration exceeds its 4 MiB limit"));}
    let mut text=String::new();let mut reader=file.take(LIMIT+1);reader.read_to_string(&mut text)?;
    let after=reader.get_ref().metadata()?;
    if text.len() as u64>LIMIT || before.len()!=after.len() || before.modified().ok()!=after.modified().ok(){return Err(io::Error::new(io::ErrorKind::InvalidData,"Configuration changed while reading"));}
    Ok(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn static_loaders_refuse_special_large_and_deep_configuration() {
        let root=tempfile::tempdir().unwrap();
        let path=root.path().join("demo.slnx");
        std::fs::write(&path,"<Solution><Project Path=\"app/app.csproj\" /></Solution>").unwrap();
        assert_eq!(crate::Solution::load(&path).unwrap(),crate::Solution::from_text(&path,&read(&path).unwrap()).unwrap());
        let deep=format!("<Solution><Folder Name=\"/{}\" /></Solution>","nested/".repeat(1000));
        assert!(crate::Solution::from_text(&path,&deep).is_err());
        let big=std::fs::File::create(&path).unwrap();big.set_len(LIMIT+1).unwrap();
        assert!(crate::Solution::load(&path).is_err());
        assert!(crate::ProjectInfo::load(&path).is_err());
        #[cfg(unix)] {
            use std::os::unix::ffi::OsStrExt;
            let fifo=root.path().join("fifo.csproj");let name=std::ffi::CString::new(fifo.as_os_str().as_bytes()).unwrap();
            assert_eq!(unsafe{libc::mkfifo(name.as_ptr(),0o600)},0);
            let start=std::time::Instant::now();
            assert!(crate::ProjectInfo::load(&fifo).is_err());
            assert!(crate::Solution::load(&fifo).is_err());
            let properties=root.path().join("Properties");std::fs::create_dir(&properties).unwrap();
            std::fs::rename(&fifo,properties.join("launchSettings.json")).unwrap();
            assert!(crate::LaunchSettings::load(&root.path().join("app.csproj")).is_err());
            assert!(start.elapsed()<std::time::Duration::from_secs(1));
        }
    }
}
