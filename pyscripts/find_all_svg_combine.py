# Promt:
# make a script on python that finds all svg files in the directory one level up and all it's children 
# and combines into one file with names of files and content of svg as svg and saves it
# make them in 3 columns and as many rows as needed
# do not add combined.svg to the results


import os


def find_svg_files(directory):
    """Find all SVG files in the specified directory and its child directories."""
    svg_files = []
    
    for dirpath, _, filenames in os.walk(directory):
        for filename in filenames:
            if filename.endswith('.svg') and filename != "combined.svg":
                svg_files.append(os.path.join(dirpath, filename))
                
    return svg_files


def combine_svg_files(svg_files):
    """Combine the contents of multiple SVG files into one."""
    combined_content = ["<svg xmlns='http://www.w3.org/2000/svg'>"]  # Start of the SVG content
    
    # Set up some constants for positioning
    SVG_WIDTH = 100  # Assuming each SVG is 300 units wide. Adjust as needed.
    SVG_HEIGHT = 100  # Assuming each SVG is 300 units tall. Adjust as needed.
    MARGIN = 5  # Spacing between SVGs
    
    col, row = 0, 0

    for svg_file in svg_files:
        with open(svg_file, 'r') as f:
            content = f.read()
            
            # Remove the outer <svg> and </svg> tags
            content = content.split('<svg', 1)[-1]  # Remove everything before (and including) the first <svg
            content = content.rsplit('</svg>', 1)[0]  # Remove everything after (and including) the last </svg>
            
            # Calculate the position for this SVG
            x_offset = col * (SVG_WIDTH + MARGIN)
            y_offset = row * (SVG_HEIGHT + MARGIN)

            # Wrap the SVG content in a <g> tag with a transform to move it to the correct position
            wrapped_content = f'<g transform="translate({x_offset}, {y_offset})"><svg{content}</svg></g>'
            
            # Add filename as comment and append the wrapped SVG content
            combined_content.append(f"<!-- {svg_file} -->\n{wrapped_content}")
            
            # Update column and row for the next SVG
            col += 1
            if col >= 4:
                col = 0
                row += 1
            
    combined_content.append("</svg>")  # End of the SVG content
    return '\n'.join(combined_content)



def main():
    directory = os.path.join(os.getcwd(), '..')  # One level up from the current directory
    svg_files = find_svg_files(directory)
    
    combined_content = combine_svg_files(svg_files)
    
    with open('combined.svg', 'w') as f:
        f.write(combined_content)
        
    print("SVG files combined and saved as 'combined.svg'")

if __name__ == '__main__':
    main()
