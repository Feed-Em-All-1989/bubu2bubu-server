use tokio::io::{AsyncRead, AsyncWrite, AsyncReadExt, AsyncWriteExt};
use tokio::sync::Mutex;
use crate::noise::NoiseResponder;

const MAX_NOISE_PAYLOAD: usize = 65000;

pub async fn send_frame<W: AsyncWrite + Unpin>(writer: &mut W, data: &[u8]) -> Result<(), String> {
    let len = (data.len() as u32).to_be_bytes();
    writer.write_all(&len).await.map_err(|e| e.to_string())?;
    writer.write_all(data).await.map_err(|e| e.to_string())?;
    writer.flush().await.map_err(|e| e.to_string())?;
    Ok(())
}

pub async fn recv_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<Vec<u8>, String> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await.map_err(|e| e.to_string())?;
    let len = u32::from_be_bytes(len_buf) as usize;

    if len > 16 * 1024 * 1024 {
        return Err("frame too large".into());
    }

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await.map_err(|e| e.to_string())?;
    Ok(buf)
}

pub async fn send_noise_msg<W: AsyncWrite + Unpin>(
    writer: &mut W,
    transport: &Mutex<NoiseResponder>,
    data: &[u8],
) -> Result<(), String> {
    let num_chunks = ((data.len() + MAX_NOISE_PAYLOAD - 1) / MAX_NOISE_PAYLOAD) as u32;
    let header = num_chunks.to_be_bytes();
    let enc_header = transport.lock().await.encrypt(&header)?;
    send_frame(writer, &enc_header).await?;
    for chunk in data.chunks(MAX_NOISE_PAYLOAD) {
        let encrypted = transport.lock().await.encrypt(chunk)?;
        send_frame(writer, &encrypted).await?;
    }
    Ok(())
}

pub async fn recv_noise_msg<R: AsyncRead + Unpin>(
    reader: &mut R,
    transport: &Mutex<NoiseResponder>,
) -> Result<Vec<u8>, String> {
    let enc_header = recv_frame(reader).await?;
    let header = transport.lock().await.decrypt(&enc_header)?;
    if header.len() < 4 {
        return Err("bad header".into());
    }
    let num_chunks = u32::from_be_bytes([header[0], header[1], header[2], header[3]]) as usize;
    let mut data = Vec::new();
    for _ in 0..num_chunks {
        let encrypted = recv_frame(reader).await?;
        let chunk = transport.lock().await.decrypt(&encrypted)?;
        data.extend_from_slice(&chunk);
    }
    Ok(data)
}
