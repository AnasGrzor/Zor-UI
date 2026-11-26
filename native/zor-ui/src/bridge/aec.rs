//! Echo cancellation for the microphone. Windows' Voice Capture DSP (CWMAudioAEC) records the mic itself and takes out
//! what the speakers play at the same time, so music (its singer too) and the buddy's own voice don't end up in what it
//! hears. It gives 16 kHz mono, the same as the plain recorder in listen.rs.

use std::{cell::{Cell, UnsafeCell}, mem::ManuallyDrop};
use windows::{
    core::{implement, Interface, Result, GUID, PROPVARIANT},
    Win32::{
        Foundation::{BOOL, E_INVALIDARG},
        Media::{Audio::*, DxMediaObjects::*},
        System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_INPROC_SERVER},
        UI::Shell::PropertiesSystem::{IPropertyStore, PROPERTYKEY},
    },
};

const CLSID_AEC: GUID = GUID::from_u128(0x745057c7_f353_4f2d_a7ee_58434477730e);
const MEDIATYPE_AUDIO: GUID = GUID::from_u128(0x73647561_0000_0010_8000_00aa00389b71);
const SUBTYPE_PCM: GUID = GUID::from_u128(0x00000001_0000_0010_8000_00aa00389b71);
const FORMAT_WAVEFORMATEX: GUID = GUID::from_u128(0x05589f81_c356_11ce_bf01_00aa0055595a);
/// The MFPKEY_WMAAECMA_* keys of wmcodecdsp.h: one GUID, property ids from PID_FIRST_USABLE (2) up.
const fn key(pid: u32) -> PROPERTYKEY { PROPERTYKEY { fmtid: GUID::from_u128(0x6f52c567_0360_4bd2_9617_ccbf1421c939), pid } }
const SYSTEM_MODE: PROPERTYKEY = key(2);
const DEVICE_INDEXES: PROPERTYKEY = key(4);
/// AEC_SYSTEM_MODE: one microphone, echo cancellation on.
const SINGLE_CHANNEL_AEC: i32 = 0;
pub const RATE: u32 = 16000;
/// One second of 16-bit mono; the DSP hands over 10 ms at a time.
const BUF: usize = RATE as usize * 2;

/// The buffer ProcessOutput writes into.
#[implement(IMediaBuffer)]
struct Buf { data: UnsafeCell<Box<[u8]>>, len: Cell<u32> }

impl IMediaBuffer_Impl for Buf_Impl {
    fn SetLength(&self, n: u32) -> Result<()> {
        if n as usize > BUF { return Err(E_INVALIDARG.into()); }
        self.len.set(n);
        Ok(())
    }
    fn GetMaxLength(&self) -> Result<u32> { Ok(BUF as u32) }
    fn GetBufferAndLength(&self, p: *mut *mut u8, n: *mut u32) -> Result<()> {
        unsafe {
            if !p.is_null() { *p = (*self.data.get()).as_mut_ptr(); }
            if !n.is_null() { *n = self.len.get(); }
        }
        Ok(())
    }
}

unsafe fn id(d: &IMMDevice) -> Result<String> {
    let p = d.GetId()?;
    let s = p.to_string().unwrap_or_default();
    CoTaskMemFree(Some(p.0 as _));
    Ok(s)
}

/// `d`'s place among the active devices of `flow`, the index DEVICE_INDEXES wants.
unsafe fn index(en: &IMMDeviceEnumerator, flow: EDataFlow, d: &IMMDevice) -> Result<i32> {
    let (list, want) = (en.EnumAudioEndpoints(flow, DEVICE_STATE_ACTIVE)?, id(d)?);
    for i in 0..list.GetCount()? {
        if id(&list.Item(i)?)? == want { return Ok(i as i32); }
    }
    Err(E_INVALIDARG.into())
}

/// The packed DEVICE_INDEXES value: the speaker's index in the high word, the microphone's in the low word.
pub fn device_indexes(speaker: i32, mic: i32) -> i32 { (speaker << 16) | (mic & 0xffff) }

pub(super) struct Aec { dmo: IMediaObject, buf: IMediaBuffer }

impl Aec {
    /// Records `mic`, minus what the default speakers play.
    pub unsafe fn open(en: &IMMDeviceEnumerator, mic: &IMMDevice) -> Result<Aec> {
        let dmo: IMediaObject = CoCreateInstance(&CLSID_AEC, None, CLSCTX_INPROC_SERVER)?;
        let props: IPropertyStore = dmo.cast()?;
        props.SetValue(&SYSTEM_MODE, &PROPVARIANT::from(SINGLE_CHANNEL_AEC))?;
        let speaker = en.GetDefaultAudioEndpoint(eRender, eConsole)?;
        props.SetValue(&DEVICE_INDEXES, &PROPVARIANT::from(device_indexes(index(en, eRender, &speaker)?, index(en, eCapture, mic)?)))?;
        let wf = WAVEFORMATEX { wFormatTag: 1 /* WAVE_FORMAT_PCM */, nChannels: 1, nSamplesPerSec: RATE, nAvgBytesPerSec: RATE * 2, nBlockAlign: 2, wBitsPerSample: 16, cbSize: 0 };
        let mt = DMO_MEDIA_TYPE {
            majortype: MEDIATYPE_AUDIO, subtype: SUBTYPE_PCM, bFixedSizeSamples: BOOL(1), bTemporalCompression: BOOL(0), lSampleSize: 2,
            formattype: FORMAT_WAVEFORMATEX, pUnk: ManuallyDrop::new(None), cbFormat: size_of::<WAVEFORMATEX>() as u32, pbFormat: &wf as *const _ as *mut u8,
        };
        dmo.SetOutputType(0, Some(&mt), 0)?;
        dmo.AllocateStreamingResources()?;
        let buf: IMediaBuffer = Buf { data: UnsafeCell::new(vec![0; BUF].into_boxed_slice()), len: Cell::new(0) }.into();
        Ok(Aec { dmo, buf })
    }

    /// Adds the audio the DSP has ready to `out`, as floats.
    pub unsafe fn read(&self, out: &mut Vec<f32>) -> Result<()> {
        loop {
            self.buf.SetLength(0)?;
            let mut b = [DMO_OUTPUT_DATA_BUFFER { pBuffer: ManuallyDrop::new(Some(self.buf.clone())), dwStatus: 0, rtTimestamp: 0, rtTimelength: 0 }];
            let mut status = 0;
            let r = self.dmo.ProcessOutput(0, &mut b, &mut status);
            ManuallyDrop::drop(&mut b[0].pBuffer);
            r?;
            let (mut p, mut n) = (std::ptr::null_mut(), 0u32);
            self.buf.GetBufferAndLength(Some(&mut p), Some(&mut n))?;
            if !p.is_null() { out.extend(pcm16(std::slice::from_raw_parts(p, n as usize))); }
            if b[0].dwStatus & DMO_OUTPUT_DATA_BUFFERF_INCOMPLETE.0 as u32 == 0 { return Ok(()); }
        }
    }
}

impl Drop for Aec {
    fn drop(&mut self) { unsafe { let _ = self.dmo.FreeStreamingResources(); } }
}

/// 16-bit little-endian PCM as floats in -1..1. A trailing odd byte is dropped.
pub fn pcm16(b: &[u8]) -> impl Iterator<Item = f32> + '_ {
    b.chunks_exact(2).map(|s| i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_indexes_pack_the_speaker_high_and_the_mic_low() {
        assert_eq!(device_indexes(0, 0), 0);
        assert_eq!(device_indexes(2, 1), 0x0002_0001);
    }

    #[test]
    fn pcm16_reads_little_endian_and_drops_an_odd_byte() {
        let v: Vec<f32> = pcm16(&[0x00, 0x40, 0x00, 0x80, 0x7f]).collect();
        assert_eq!(v, [0.5, -1.0]);
    }
}
