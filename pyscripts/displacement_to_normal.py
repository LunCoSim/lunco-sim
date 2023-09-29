import numpy as np
from PIL import Image

def displacement_to_normal(displacement_image_path, normal_image_path):
    # Load the displacement map
    displacement = np.array(Image.open(displacement_image_path))

    # Compute the gradients in x and y directions
    dx = np.gradient(displacement, axis=1)
    dy = np.gradient(displacement, axis=0)

    # Normalize the gradients to get the normals
    norm = np.sqrt(dx**2 + dy**2 + 1)
    nx = -dx / norm
    ny = -dy / norm
    nz = 1.0 / norm

    # Convert normals from [-1, 1] to [0, 255]
    r = ((nx + 1) * 0.5 * 255).astype(np.uint8)
    g = ((ny + 1) * 0.5 * 255).astype(np.uint8)
    b = ((nz + 1) * 0.5 * 255).astype(np.uint8)

    # Stack the channels together
    normal_map = np.stack((r, g, b), axis=-1)

    # Save the normal map
    Image.fromarray(normal_map).save(normal_image_path)

# Example usage
#displacement_to_normal('path_to_tiff_displacement_map.tiff', 'output_normal_map.png')

# Example usage:
displacement_to_normal('ldem_16.tif', 'ldem_16.png')
