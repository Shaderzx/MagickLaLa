pub fn cosine(left: &[f32], right: &[f32]) -> f32 {
    if left.len() != right.len() {
        return 0.0;
    }
    let dot: f32 = left.iter().zip(right).map(|(a, b)| a * b).sum();
    let norms = left.iter().map(|a| a * a).sum::<f32>().sqrt() * right.iter().map(|b| b * b).sum::<f32>().sqrt();
    if norms > 0.0 { dot / norms } else { 0.0 }
}
